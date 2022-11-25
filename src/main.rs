use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use indicatif::{ProgressBar, ProgressIterator};
use rand::distributions::Alphanumeric;
use rand::Rng;
use std::cmp::max;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::str;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::{thread, time};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};

const TERM_ENDC: &str = "\x1b[0m";
const TERM_BOLD: &str = "\x1b[1m";
const TERM_ERR: &str = "\x1b[0m\x1b[91m";
const TERM_OK: &str = "\x1b[0m\x1b[92m";

const SIDE: usize = 128;
// const SPEED: u64 = 200;
// const START_WAIT: u64 = 5000;
const GAME_CLOCK_INTERVAL: u32 = 5;
const GAME_STEPS: usize = 2048;
const UPDATE_FREQ: u64 = 0;
const NC_WAIT_SECS: u64 = 1;
const CHAR_MAP: [&str; 5] = [
    "  ",
    "\x1b[41m  \x1b[0m",
    "\x1b[42m  \x1b[0m",
    "\x1b[44m  \x1b[0m",
    "\x1b[46m  \x1b[0m",
];
const WHITE: &str = "\x1b[47m  \x1b[0m";
const STRAT_DIR_NAME: &str = "strats";

struct Game {
    i: usize,
    boards: [u8; SIDE * SIDE * GAME_STEPS],
}

fn toilet(to_toilet: String) -> String {
    str::from_utf8(
        &Command::new("toilet")
            .args(["-f", "mono12", "-w", "10000", &to_toilet])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .to_string()
}

async fn process(mut socket: TcpStream, cur_game: Arc<RwLock<Game>>, strat_dir: &str) {
    let mut buf = [0u8; (32 + (SIDE / 2) * (SIDE / 2)) * 2];
    if let Ok(Ok(amount)) = timeout(Duration::from_millis(1000), socket.read(&mut buf)).await {
        let token = &buf[..32];
        let hex_token = token
            .iter()
            .fold(String::new(), |x, y| x + &(*y as char).to_string());
        let mut token_match = false;
        for path in fs::read_dir(strat_dir).unwrap().flatten() {
            if path.file_name().to_str().unwrap() == hex_token {
                token_match = true;
                let content = buf[32..amount]
                    .iter()
                    .fold(String::new(), |x, y| x + &(*y as char).to_string());

                if content.replace('\n', "").len() == (SIDE / 2) * (SIDE / 2) {
                    let log = format!(
                        "{}Updated strat for \"{}{}{}\"{}",
                        TERM_OK, TERM_BOLD, hex_token, TERM_OK, TERM_ENDC
                    );
                    fs::write(path.path(), content).unwrap();
                    println!("{}", log);
                    socket.write_all(log.as_bytes()).await.unwrap();
                    break;
                } else {
                    let err_str = format!(
                        "{}{}ERROR{}: New strategy for \"{}{}{}\" of wrong length. Got {} but expected {} (\\n are ignored){}", TERM_ERR, TERM_BOLD, TERM_ERR, TERM_BOLD,
                        hex_token, TERM_ERR,
                        (SIDE / 2) * (SIDE / 2),
                        content.replace('\n', "").len(), TERM_ENDC
                    );
                    println!("{}", err_str);
                    socket.write_all(err_str.as_bytes()).await.unwrap();
                }
            }
        }
        if !token_match {
            socket
                .write_all(format!("ERROR: Unknown token \"{}\"", hex_token).as_bytes())
                .await
                .unwrap();
        }
    }
    loop {
        let mut sorted_scores = vec![(0, 0), (1, 0), (2, 0), (3, 0)];
        for i in 0..GAME_STEPS {
            let game_offset = i * SIDE * SIDE;
            let mut scores = [0usize, 0usize, 0usize, 0usize, 0usize];
            let mut to_send = format!("{}\n", WHITE.repeat(SIDE + 2));
            {
                let rdb = cur_game.read().unwrap();

                for i in 0..SIDE {
                    let mut board_str = String::new();

                    for square in &rdb.boards[game_offset + i * SIDE..game_offset + (i + 1) * SIDE]
                    {
                        board_str += CHAR_MAP[*square as usize];
                        scores[*square as usize] += 1;
                    }
                    to_send += &format!("{}{}{}\n", WHITE, board_str, WHITE);
                }
            }
            to_send += &format!("{}\n", WHITE.repeat(SIDE + 2));
            sorted_scores = Vec::from_iter(scores[1..].iter().copied().enumerate());
            sorted_scores.sort_by(|(_, x), (_, y)| y.cmp(x));

            let cmd = toilet(
                format!(
                    "Game {} | step {:0width$}/{}\nT{}: {:04}\nT{}: {:04}\nT{}: {:04}\nT{}: {:04}",
                    { cur_game.read().unwrap().i },
                    i + 1,
                    GAME_STEPS,
                    sorted_scores[0].0,
                    sorted_scores[0].1,
                    sorted_scores[1].0,
                    sorted_scores[1].1,
                    sorted_scores[2].0,
                    sorted_scores[2].1,
                    sorted_scores[3].0,
                    sorted_scores[3].1,
                    width = GAME_STEPS.to_string().len()
                )
                .to_string(),
            );

            to_send = "\n".repeat(SIDE / 2) + &cmd + &to_send;

            if let Err(e) = socket.write_all(to_send.as_bytes()).await {
                println!("Con closed: {}", e);
                return;
            }
            thread::sleep(time::Duration::from_millis(UPDATE_FREQ));
        }
        let (sleep_seconds, at) = calculate_next_game(Duration::new(NC_WAIT_SECS, 0)).await;
        socket
            .write_all(
                toilet(
                    format!(
                        "WINNER T{}! - Next game at {:?}",
                        sorted_scores[0].0,
                        at.to_rfc3339_opts(SecondsFormat::Secs, true)
                    )
                    .to_string(),
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        sleep(sleep_seconds).await;
    }
}

fn load_strats(strat_dir: &str) -> Vec<String> {
    let mut strats = Vec::new();

    for path in fs::read_dir(strat_dir).unwrap().flatten() {
        match fs::read_to_string(path.path()) {
            Ok(mut content) => {
                content = content.replace('\n', "");
                if content.len() == (SIDE / 2) * (SIDE / 2) {
                    strats.push(content);
                } else {
                    println!(
                        "WARNING: \"{}\" not correct len ({} != {}), deleting file",
                        path.path().display(),
                        content.len(),
                        (SIDE / 2) * (SIDE / 2)
                    );
                    fs::remove_file(path.path()).unwrap();
                }
            }
            Err(e) => println!("Cannot read \"{}\": {}", path.path().display(), e),
        }
    }
    let default_strat = format!("{}\n", "0".repeat(SIDE / 2))
        + &(format!("{}\n", "0".to_owned() + &"1".repeat(SIDE / 2 - 2) + "0")).repeat(SIDE / 2 - 2)
        + &format!("{}\n", "0".repeat(SIDE / 2));

    for _ in 0..max(4 - strats.len(), 0) {
        let strat = default_strat.clone();
        File::create(format!(
            "{}/{}",
            strat_dir,
            rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(32)
                .map(char::from)
                .collect::<String>()
        ))
        .unwrap()
        .write_all(strat.as_bytes())
        .unwrap();
        strats.push(strat);
    }

    strats
}

async fn init_game(shared_game: Arc<RwLock<Game>>) {
    if let Ok(()) = fs::create_dir(STRAT_DIR_NAME) {
        println!("Created dir \"{}\"", STRAT_DIR_NAME);
    }

    let teams = &load_strats(STRAT_DIR_NAME);

    let mut game = shared_game.write().unwrap();
    for (i, t) in teams.iter().enumerate() {
        let tb = t.as_bytes();
        let i0: Vec<usize> = (0..SIDE / 2).collect();
        let i1: Vec<usize> = (SIDE / 2..SIDE).rev().collect();
        let (xr, yr) = match i {
            1 => (&i1, &i0),
            2 => (&i0, &i1),
            3 => (&i1, &i1),
            _ => (&i0, &i0),
        };
        let mut ii = 0;
        for y in yr {
            for x in xr {
                game.boards[x + y * SIDE] = if tb[ii] == b'0' { 0 } else { (i + 1) as u8 };
                ii += 1;
            }
        }
    }
}

async fn calculate_next_game(offset: Duration) -> (Duration, DateTime<Utc>) {
    let now = Utc::now();

    let sleep_seconds = Duration::from_secs(
        ((GAME_CLOCK_INTERVAL - now.minute() % GAME_CLOCK_INTERVAL - 1) * 60 + 60 - now.second())
            as u64,
    ) + offset;
    (
        sleep_seconds,
        now.checked_add_signed(chrono::Duration::from_std(sleep_seconds).unwrap())
            .unwrap(),
    )
}

async fn run_game(shared_game: Arc<RwLock<Game>>) {
    loop {
        let game = shared_game.clone();
        init_game(game).await;

        let i_side = SIDE as i32;
        let pb = ProgressBar::new(GAME_STEPS as u64).with_message("Running game...");
        let now = Instant::now();
        {
            let mut game = shared_game.write().unwrap();
            for ii in (0..GAME_STEPS - 1).progress() {
                pb.set_position(ii as u64);
                let game_offset = (ii * SIDE * SIDE) as i32;
                let next_game_offset = ((ii + 1) * SIDE * SIDE) as i32;
                {
                    for y in 0..SIDE as i32 {
                        for x in 0..SIDE as i32 {
                            let mut teams = [0, 0, 0, 0, 0];
                            for (dx, dy) in [
                                (-1, -1),
                                (-1, 0),
                                (-1, 1),
                                (0, -1),
                                (0, 1),
                                (1, -1),
                                (1, 0),
                                (1, 1),
                            ] {
                                if x + dx < i_side && x + dx >= 0 && y + dy < i_side && y + dy >= 0
                                {
                                    teams[game.boards
                                        [(game_offset + (x + dx) + (y + dy) * i_side) as usize]
                                        as usize] += 1;
                                }
                            }
                            let total_neighbours = teams[1] + teams[2] + teams[3] + teams[4];
                            if total_neighbours == 3
                                || total_neighbours == 2
                                    && game.boards[(game_offset + x + y * i_side) as usize] != 0
                            {
                                if (teams[1] > teams[2])
                                    && (teams[1] > teams[3])
                                    && (teams[1] > teams[4])
                                {
                                    game.boards[(next_game_offset + x + y * i_side) as usize] = 1;
                                } else if (teams[2] > teams[1])
                                    && (teams[2] > teams[3])
                                    && (teams[2] > teams[4])
                                {
                                    game.boards[(next_game_offset + x + y * i_side) as usize] = 2;
                                } else if (teams[3] > teams[1])
                                    && (teams[3] > teams[2])
                                    && (teams[3] > teams[4])
                                {
                                    game.boards[(next_game_offset + x + y * i_side) as usize] = 3;
                                } else if (teams[4] > teams[1])
                                    && (teams[4] > teams[2])
                                    && (teams[4] > teams[3])
                                {
                                    game.boards[(next_game_offset + x + y * i_side) as usize] = 4;
                                }
                            } else {
                                game.boards[(next_game_offset + x + y * i_side) as usize] = 0;
                            }
                        }
                    }
                }
            }
        }
        pb.finish();
        let i = { shared_game.read().unwrap().i };
        println!("Game {} generated in {}ms", i, now.elapsed().as_millis());
        let (sleep_seconds, at) = calculate_next_game(Duration::new(0, 0)).await;
        println!(
            "Sleeping for {}s ({:?}) until running next game",
            sleep_seconds.as_secs(),
            at.to_rfc3339_opts(SecondsFormat::Secs, true)
        );
        sleep(sleep_seconds).await;
        {
            shared_game.write().unwrap().i += 1;
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:2602".to_string());

    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on: {}", addr);
    let shared_game = Arc::new(RwLock::new(Game {
        i: 0,
        boards: [0u8; SIDE * SIDE * GAME_STEPS],
    }));

    let game = shared_game.clone();
    tokio::spawn(async move {
        run_game(game).await;
    });

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        // Clone the handle to the hash map.
        let db = shared_game.clone();

        println!("Accepted");
        tokio::spawn(async move {
            process(socket, db, STRAT_DIR_NAME).await;
        });
    }
}
