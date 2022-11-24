use indicatif::{ProgressBar, ProgressIterator};
use rand::distributions::Alphanumeric;
use rand::Rng;
use std::cmp::max;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::{thread, time};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const SIDE: usize = 32;
// const SPEED: u64 = 200;
// const START_WAIT: u64 = 5000;
const GAME_STEPS: usize = 1000;
const UPDATE_FREQ: u64 = 200;
const CHAR_MAP: [&str; 5] = [
    "  ",
    "\x1b[43m  \x1b[0m",
    "\x1b[44m  \x1b[0m",
    "\x1b[45m  \x1b[0m",
    "\x1b[46m  \x1b[0m",
];
const WHITE: &str = "\x1b[47m  \x1b[0m";

async fn process(mut socket: TcpStream, db: Arc<RwLock<[u8; SIDE * SIDE * GAME_STEPS]>>) {
    let mut buf = [0u8; 32 + (SIDE / 2) * (SIDE / 2)];
    if let Ok(Ok(_new_value)) =
        timeout(Duration::from_millis(1000), socket.read_exact(&mut buf)).await
    {
        println!("Got value!");
    }
    for i in 0..GAME_STEPS {
        let game_offset = i * SIDE * SIDE;
        let mut to_send: String =
            "\n".repeat(SIDE / 2) + &format!("Game step {}/{}\n", i + 1, GAME_STEPS);
        to_send += &format!("{}\n", WHITE.repeat(SIDE + 2));
        {
            let rdb = db.read().unwrap();
            for i in 0..SIDE {
                to_send += &format!(
                    "{}{}{}\n",
                    WHITE,
                    rdb[game_offset + i * SIDE..game_offset + (i + 1) * SIDE]
                        .iter()
                        .map(|x| CHAR_MAP[*x as usize])
                        .collect::<String>(),
                    WHITE
                );
            }
        }
        to_send += &format!("{}\n", WHITE.repeat(SIDE + 2));
        if let Err(e) = socket.write_all(to_send.as_bytes()).await {
            println!("Con closed: {}", e);
            return;
        }
        thread::sleep(time::Duration::from_millis(UPDATE_FREQ));
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
            "strats/{}",
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

const STRAT_DIR_NAME: &str = "strats";
async fn init_game(db: Arc<RwLock<[u8; SIDE * SIDE * GAME_STEPS]>>) {
    if let Ok(()) = fs::create_dir(STRAT_DIR_NAME) {
        println!("Created dir \"{}\"", STRAT_DIR_NAME);
    }

    let teams = &load_strats(STRAT_DIR_NAME);

    let mut mdb = db.write().unwrap();
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
                mdb[x + y * SIDE] = if tb[ii] == b'0' { 0 } else { (i + 1) as u8 };
                ii += 1;
            }
        }
    }
}

async fn run_game(db: Arc<RwLock<[u8; SIDE * SIDE * GAME_STEPS]>>) {
    let i_side = SIDE as i32;
    let pb = ProgressBar::new(GAME_STEPS as u64).with_message("Running game...");
    let now = Instant::now();
    for i in (0..GAME_STEPS - 1).progress() {
        pb.set_position(i as u64);
        let game_offset = (i * SIDE * SIDE) as i32;
        let next_game_offset = ((i + 1) * SIDE * SIDE) as i32;
        {
            let mut mdb = db.write().unwrap();
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
                        if x + dx < i_side && x + dx >= 0 && y + dy < i_side && y + dy >= 0 {
                            teams[mdb[(game_offset + (x + dx) + (y + dy) * i_side) as usize]
                                as usize] += 1;
                        }
                    }
                    let total_neighbours = teams[1] + teams[2] + teams[3] + teams[4];
                    if total_neighbours == 3
                        || total_neighbours == 2
                            && mdb[(game_offset + x + y * i_side) as usize] != 0
                    {
                        if teams[1] > teams[2] && teams[1] > teams[3] && teams[1] > teams[4] {
                            mdb[(next_game_offset + x + y * i_side) as usize] = 1;
                        } else if teams[2] > teams[1] && teams[2] > teams[3] && teams[2] > teams[4]
                        {
                            mdb[(next_game_offset + x + y * i_side) as usize] = 2;
                        } else if teams[3] > teams[1] && teams[3] > teams[2] && teams[3] > teams[4]
                        {
                            mdb[(next_game_offset + x + y * i_side) as usize] = 3;
                        } else if teams[4] > teams[1] && teams[4] > teams[2] && teams[4] > teams[3]
                        {
                            mdb[(next_game_offset + x + y * i_side) as usize] = 4;
                        }
                    } else {
                        mdb[(next_game_offset + x + y * i_side) as usize] = 0;
                    }
                }
            }
        }
    }
    pb.finish();
    println!("Game generated in {}ms", now.elapsed().as_millis());
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:2601".to_string());

    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on: {}", addr);
    let db = Arc::new(RwLock::new([0u8; SIDE * SIDE * GAME_STEPS]));

    let tmp_db = db.clone();
    init_game(tmp_db).await;

    let tmp_db = db.clone();
    tokio::spawn(async move {
        run_game(tmp_db).await;
    });

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        // Clone the handle to the hash map.
        let db = db.clone();

        println!("Accepted");
        tokio::spawn(async move {
            process(socket, db).await;
        });
    }
}
