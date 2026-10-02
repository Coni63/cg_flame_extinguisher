//! Referee / game engine for "Extinction des feux".
//!
//! Usage: referee <test_file> <bot_command> [--seed N] [--timeout MS] [-v]
//!
//! The referee spawns the bot, sends the init + per-turn inputs exactly like
//! CodinGame, reads the action line, simulates it and prints one final line on
//! stdout:  RESULT status=WIN score=31 cycles=8 turns=20 max_ms=2
//! (score = remaining HP on victory, 0 otherwise).
//!
//! Test file format = the CodinGame test dump (map + first turn inputs). It may
//! optionally be followed by extra glyph groups ("k" then k lines "x y") that
//! describe the spawns of cycles 2, 3, ... Missing groups are generated randomly
//! (seeded) because the statement does not give the spawn rule.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

type P = (i32, i32);

const MAX_AP: i32 = 10;
const MAX_JUMPS: i32 = 2;
const SPAWN_CYCLES: u32 = 6;
const GLYPHS_PER_SPAWN: usize = 6;
const MAX_TURNS: u32 = 2000;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Running,
    Won,
    Lost(String),
}

#[derive(Clone)]
pub struct Game {
    pub w: i32,
    pub h: i32,
    walls: Vec<bool>,
    map_rows: Vec<String>,
    pub pos: P,
    pub hp: i32,
    pub ap: i32,
    pub jumps_left: i32,
    pub flames: Vec<P>,
    pub glyphs: Vec<P>,
    pub cycle: u32,
    scripted: Vec<Vec<P>>,
    rng: u64,
    pub status: Status,
}

/// Diagonal push blocked by: 0 nothing, 1 both corners, 2 vertical corner, 3 horizontal corner,
/// 4 diagonal flames are not pushed at all (CodinGame behaviour: the attraction then pulls them
/// onto the character, which dies). Overridable with the FLAME_CORNER env var.
fn corner_mode() -> u8 {
    std::env::var("FLAME_CORNER").ok().and_then(|c| c.parse().ok()).unwrap_or(4)
}

/// Neighbours are pushed only when the move extinguished a flame (observed on CodinGame).
/// FLAME_PUSHX=0 restores the README behaviour (always pushed).
fn push_only_on_extinguish() -> bool {
    std::env::var("FLAME_PUSHX").map_or(true, |v| v != "0")
}

fn dir_vec(d: &str) -> Option<P> {
    Some(match d {
        "N" => (0, -1),
        "S" => (0, 1),
        "E" => (1, 0),
        "W" => (-1, 0),
        "NE" => (1, -1),
        "NW" => (-1, -1),
        "SE" => (1, 1),
        "SW" => (-1, 1),
        _ => return None,
    })
}

fn tok<'a>(t: &mut impl Iterator<Item = &'a str>) -> Result<i32, String> {
    t.next()
        .ok_or("unexpected end of test file")?
        .parse::<i32>()
        .map_err(|e| e.to_string())
}

fn points<'a>(t: &mut impl Iterator<Item = &'a str>) -> Result<Vec<P>, String> {
    let n: i32 = tok(t)?;
    (0..n).map(|_| Ok((tok(t)?, tok(t)?))).collect()
}

impl Game {
    pub fn parse(text: &str, seed: u64) -> Result<Game, String> {
        let mut t = text.split_whitespace();
        let h: i32 = tok(&mut t)?;
        let w: i32 = tok(&mut t)?;
        let mut walls = vec![false; (w * h) as usize];
        let mut map_rows = Vec::new();
        for y in 0..h {
            let row = t.next().ok_or("missing map row")?;
            if row.len() != w as usize {
                return Err(format!("map row {y} has wrong length"));
            }
            for (x, c) in row.chars().enumerate() {
                walls[(y * w) as usize + x] = c == '#';
            }
            map_rows.push(row.replace('S', "."));
        }
        let pos = (tok(&mut t)?, tok(&mut t)?);
        let (hp, ap, jumps_left) = (tok(&mut t)?, tok(&mut t)?, tok(&mut t)?);
        let flames = points(&mut t)?;
        let glyphs = points(&mut t)?;
        // listed valid actions: PASS is one token, the others are two
        let n_valid: i32 = tok(&mut t)?;
        for _ in 0..n_valid {
            if t.next().ok_or("missing action")? != "PASS" {
                t.next().ok_or("missing action dir")?;
            }
        }
        // optional extra glyph groups for cycles 2, 3, ...
        let mut scripted = Vec::new();
        while t.clone().next().is_some() {
            scripted.push(points(&mut t)?);
        }
        Ok(Game {
            w,
            h,
            walls,
            map_rows,
            pos,
            hp,
            ap,
            jumps_left,
            flames,
            glyphs,
            cycle: 1,
            scripted,
            rng: seed.wrapping_mul(0x9E3779B97F4A7C15) | 1,
            status: Status::Running,
        })
    }

    fn is_wall(&self, p: P) -> bool {
        p.0 < 0 || p.1 < 0 || p.0 >= self.w || p.1 >= self.h || self.walls[(p.1 * self.w + p.0) as usize]
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// Init block sent once to the bot (the map has no 'S', start is in the turn input).
    pub fn init_text(&self) -> String {
        let mut s = format!("{} {}\n", self.h, self.w);
        for r in &self.map_rows {
            s.push_str(r);
            s.push('\n');
        }
        s
    }

    pub fn valid_actions(&self) -> Vec<String> {
        let mut v = vec!["PASS".to_string()];
        for (name, (dx, dy)) in [("N", (0, -1)), ("E", (1, 0)), ("S", (0, 1)), ("W", (-1, 0))] {
            if self.ap >= 1 && !self.is_wall((self.pos.0 + dx, self.pos.1 + dy)) {
                v.push(format!("MOVE {name}"));
            }
            if self.ap >= 2
                && self.jumps_left > 0
                && !self.is_wall((self.pos.0 + 2 * dx, self.pos.1 + 2 * dy))
            {
                v.push(format!("JUMP {name}"));
            }
        }
        if self.ap >= 1 {
            for d in ["NE", "SE", "SW", "NW"] {
                v.push(format!("IMMO {d}"));
            }
        }
        v
    }

    /// Inputs of the current turn, exactly as the bot reads them.
    pub fn turn_text(&self) -> String {
        let mut s = format!(
            "{} {}\n{} {} {}\n{}\n",
            self.pos.0, self.pos.1, self.hp, self.ap, self.jumps_left, self.flames.len()
        );
        for f in &self.flames {
            s.push_str(&format!("{} {}\n", f.0, f.1));
        }
        s.push_str(&format!("{}\n", self.glyphs.len()));
        for g in &self.glyphs {
            s.push_str(&format!("{} {}\n", g.0, g.1));
        }
        let va = self.valid_actions();
        s.push_str(&format!("{}\n", va.len()));
        for a in va {
            s.push_str(&a);
            s.push('\n');
        }
        s
    }

    fn lose(&mut self, why: impl Into<String>) {
        if self.status == Status::Running {
            self.status = Status::Lost(why.into());
        }
    }

    fn check_win(&mut self) {
        if self.status == Status::Running
            && self.flames.is_empty()
            && self.glyphs.is_empty()
            && self.cycle > SPAWN_CYCLES
        {
            self.status = Status::Won;
        }
    }

    /// Plays one output line of the bot (actions separated by ';').
    pub fn play_line(&mut self, line: &str) {
        let mut any = false;
        for raw in line.split(';') {
            let a = raw.trim();
            if a.is_empty() {
                continue;
            }
            any = true;
            if !self.valid_actions().iter().any(|v| v == a) {
                self.lose(format!("illegal action '{a}'"));
                return;
            }
            if a == "PASS" {
                self.pass();
                return; // everything after PASS is ignored
            }
            self.apply(a);
            if self.status != Status::Running {
                return;
            }
        }
        if !any {
            self.lose("empty output");
        }
    }

    fn apply(&mut self, a: &str) {
        let mut it = a.split_whitespace();
        let kind = it.next().unwrap();
        let (dx, dy) = dir_vec(it.next().unwrap()).unwrap();
        match kind {
            "MOVE" | "JUMP" => {
                let dist = if kind == "MOVE" { 1 } else { 2 };
                if self.hp <= 1 {
                    self.hp = 0;
                    return self.lose("died moving with 1 HP");
                }
                self.hp -= 1;
                self.ap -= dist;
                if kind == "JUMP" {
                    self.jumps_left -= 1;
                }
                self.pos = (self.pos.0 + dx * dist, self.pos.1 + dy * dist);
                let extinguished = self.flames.iter().position(|&f| f == self.pos).map(|i| {
                    self.flames.swap_remove(i);
                    self.hp += 1; // refund
                });
                // CodinGame only pushes the neighbours when a flame was extinguished: after a move
                // onto an empty cell an adjacent flame is pulled onto the character
                if (extinguished.is_some() || !push_only_on_extinguish()) && !self.push_adjacent() {
                    return self.lose("flame blocked while pushing");
                }
                if !self.attract(self.pos, false) {
                    return self.lose("flame pulled onto the character");
                }
            }
            _ => {
                // IMMO
                self.ap -= 1;
                self.hp -= 5;
                if self.hp <= 0 {
                    return self.lose("died using IMMO");
                }
                let target = (self.pos.0 + dx, self.pos.1 + dy);
                self.attract(target, true);
            }
        }
        self.check_win();
    }

    /// Pushes the 8 neighbours of the player away. Returns false if one is blocked.
    fn push_adjacent(&mut self) -> bool {
        let orig = self.flames.clone();
        for f in self.flames.iter_mut() {
            let (rx, ry) = (f.0 - self.pos.0, f.1 - self.pos.1);
            if rx.abs().max(ry.abs()) != 1 {
                continue;
            }
            if rx != 0 && ry != 0 && corner_mode() == 4 {
                continue; // not pushed: the attraction will pull it onto the character
            }
            let dest = (f.0 + rx, f.1 + ry);
            let blocked = |c: P| {
                c.0 < 0
                    || c.1 < 0
                    || c.0 >= self.w
                    || c.1 >= self.h
                    || self.walls[(c.1 * self.w + c.0) as usize]
                    || orig.contains(&c)
            };
            // a diagonal push is also blocked by its two corner cells (observed on CodinGame)
            let corner = rx != 0
                && ry != 0
                && match corner_mode() {
                    0 => false,
                    1 => blocked((f.0 + rx, f.1)) || blocked((f.0, f.1 + ry)),
                    2 => blocked((f.0, f.1 + ry)),
                    _ => blocked((f.0 + rx, f.1)),
                };
            if corner || blocked(dest) {
                return false;
            }
            *f = dest;
        }
        true
    }

    /// Pulls every flame one step toward `target`. When `immo` is true the player
    /// cell is a wall; otherwise a flame reaching the player kills (returns false).
    fn attract(&mut self, target: P, immo: bool) -> bool {
        let angle = |f: &P| {
            let a = ((f.0 - target.0) as f64).atan2(-(f.1 - target.1) as f64);
            if a < 0.0 { a + std::f64::consts::TAU } else { a }
        };
        let mut order: Vec<P> = self.flames.clone();
        order.sort_by(|a, b| {
            let da = (a.0 - target.0).abs() + (a.1 - target.1).abs();
            let db = (b.0 - target.0).abs() + (b.1 - target.1).abs();
            da.cmp(&db).then(angle(a).partial_cmp(&angle(b)).unwrap())
        });
        for f in order {
            let (ddx, ddy) = (target.0 - f.0, target.1 - f.1);
            let (ax, ay) = (ddx.abs(), ddy.abs());
            let step = if ax == 0 && ay == 0 {
                continue;
            } else if ax == 0 {
                (0, ddy.signum())
            } else if ay == 0 {
                (ddx.signum(), 0)
            } else if ax == ay {
                (ddx.signum(), ddy.signum())
            } else if ax > ay {
                (ddx.signum(), 0)
            } else {
                (0, ddy.signum())
            };
            let dest = (f.0 + step.0, f.1 + step.1);
            if self.is_wall(dest) || self.flames.contains(&dest) {
                continue;
            }
            if dest == self.pos {
                if immo {
                    continue;
                }
                return false;
            }
            let i = self.flames.iter().position(|&x| x == f).unwrap();
            self.flames[i] = dest;
        }
        true
    }

    fn pass(&mut self) {
        // glyphs -> flames, unless the player or a flame is on the cell
        for g in std::mem::take(&mut self.glyphs) {
            if g != self.pos && !self.flames.contains(&g) {
                self.flames.push(g);
            }
        }
        self.ap = MAX_AP;
        self.jumps_left = MAX_JUMPS;
        self.cycle += 1;
        let spawn = if let Some(g) = self.scripted.get(self.cycle as usize - 2) {
            g.clone()
        } else if self.cycle <= SPAWN_CYCLES {
            self.random_glyphs()
        } else {
            Vec::new()
        };
        if spawn.is_empty() {
            if self.flames.is_empty() && self.cycle > SPAWN_CYCLES {
                self.status = Status::Won;
                return;
            }
            self.hp -= 1;
            if self.hp <= 0 {
                self.lose("no glyph spawned, out of HP");
            }
        }
        self.glyphs = spawn;
    }

    fn random_glyphs(&mut self) -> Vec<P> {
        let mut v: Vec<P> = Vec::new();
        let mut tries = 0;
        while v.len() < GLYPHS_PER_SPAWN && tries < 10_000 {
            tries += 1;
            let p = (
                (self.rand() % self.w as u64) as i32,
                (self.rand() % self.h as u64) as i32,
            );
            if !self.is_wall(p) && p != self.pos && !self.flames.contains(&p) && !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: referee <test_file> <bot_command> [--seed N] [--timeout MS] [-v]");
        std::process::exit(2);
    }
    let (mut seed, mut timeout_ms, mut verbose) = (1u64, 100u64, false);
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => {
                seed = args[i + 1].parse().expect("seed");
                i += 1;
            }
            "--timeout" => {
                timeout_ms = args[i + 1].parse().expect("timeout");
                i += 1;
            }
            "-v" => verbose = true,
            other => panic!("unknown arg {other}"),
        }
        i += 1;
    }
    let text = std::fs::read_to_string(&args[1]).expect("cannot read test file");
    let mut game = Game::parse(&text, seed).expect("bad test file");

    let mut child = Command::new(&args[2])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if verbose { Stdio::inherit() } else { Stdio::null() })
        .spawn()
        .expect("cannot start bot");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut send = |s: &str| -> bool { stdin.write_all(s.as_bytes()).is_ok() && stdin.flush().is_ok() };
    let mut turns = 0u32;
    let mut max_ms = 0u128;
    let mut ok = send(&game.init_text());
    while ok && game.status == Status::Running {
        if turns >= MAX_TURNS {
            game.status = Status::Lost("too many turns".into());
            break;
        }
        turns += 1;
        let t0 = Instant::now();
        if verbose {
            eprintln!("--- turn {turns} (cycle {}) ---\n{}", game.cycle, game.turn_text());
        }
        if !send(&game.turn_text()) {
            game.status = Status::Lost("bot closed stdin".into());
            break;
        }
        // first turn includes process startup
        let limit = if turns == 1 { timeout_ms.max(1000) } else { timeout_ms };
        match rx.recv_timeout(Duration::from_millis(limit)) {
            Ok(line) => {
                max_ms = max_ms.max(t0.elapsed().as_millis());
                if verbose {
                    eprintln!("> {line}");
                }
                game.play_line(&line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => game.status = Status::Lost("timeout".into()),
            Err(_) => game.status = Status::Lost("bot exited".into()),
        }
        ok = true;
    }
    let _ = child.kill();
    let _ = child.wait();

    let (name, score) = match &game.status {
        Status::Won => ("WIN".to_string(), game.hp),
        Status::Lost(why) => (format!("LOSE({})", why.replace(' ', "_")), 0),
        Status::Running => ("ERROR".to_string(), 0),
    };
    println!(
        "RESULT status={name} score={score} hp={} cycles={} turns={turns} max_ms={max_ms}",
        game.hp, game.cycle
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut s = String::from("7 9\n");
        for y in 0..7 {
            s.push_str(if y == 0 || y == 6 { "#########\n" } else { "#.......#\n" });
        }
        s.push_str("4 3\n40 10 2\n0\n0\n0\n");
        Game::parse(&s, 1).unwrap()
    }

    #[test]
    fn move_costs_hp_and_extinguishes_with_refund() {
        let mut g = game();
        g.flames = vec![(5, 3)];
        g.play_line("MOVE E");
        assert_eq!(g.pos, (5, 3));
        assert_eq!(g.hp, 40);
        assert!(g.flames.is_empty());
    }

    #[test]
    fn blocked_push_kills() {
        let mut g = game();
        g.pos = (1, 3);
        g.flames = vec![(0, 1)]; // after MOVE N to (1,2) it is adjacent and pushed into the wall
        g.play_line("MOVE N");
        assert!(matches!(g.status, Status::Lost(_)));
    }

    #[test]
    fn diagonal_push_blocked_by_corner() {
        // CodinGame game: MOVE S from (13,10) with flames at (12,11) (14,12) (12,12) (12,13)
        // killed the character: (12,12) -> (11,13) is blocked by the flame on (12,13).
        let mut g = game();
        g.pos = (4, 1);
        g.flames = vec![(3, 2), (5, 3), (3, 3), (3, 4)];
        g.play_line("MOVE S");
        assert!(matches!(g.status, Status::Lost(_)));
    }

    #[test]
    fn diagonal_flame_is_fatal() {
        // CodinGame game: MOVE S from (20,18) to (20,19) with a lone flame at (21,18) killed
        // the character although its diagonal push had every cell free.
        let mut g = game();
        g.pos = (4, 2);
        g.flames = vec![(5, 2)];
        g.play_line("MOVE S");
        assert!(matches!(g.status, Status::Lost(_)));
    }

    #[test]
    fn no_push_without_extinguish() {
        // CodinGame game: JUMP N from (15,14) to the empty cell (15,12) over a flame on (15,13)
        // killed the character: nothing is pushed, the flame is pulled onto it.
        let mut g = game();
        g.pos = (4, 4);
        g.flames = vec![(4, 3)];
        g.play_line("JUMP N");
        assert!(matches!(g.status, Status::Lost(_)));
    }

    #[test]
    fn attraction_moves_flame_one_step() {
        let mut g = game();
        g.flames = vec![(7, 3)];
        g.play_line("IMMO NE");
        assert_eq!(g.hp, 35);
        assert_eq!(g.flames[0], (6, 3));
    }
}
