//! Solver for "Extinction des feux".
//!
//! Exact re-implementation of the referee rules + a time-bucketed beam search
//! re-run at every turn (one action is sent per turn). While glyph spawns are
//! still to come, the horizon is the end of the next cycle (everything up to
//! that point is known: future glyphs only matter at its PASS). From cycle 6
//! on the game is fully deterministic and the horizon covers several cycles.

use std::io::{self, BufRead};
use std::time::Instant;

const W: i32 = 35;
const H: i32 = 34;
const PAD: i32 = 2;
const PW: i32 = W + 2 * PAD;
const PH: i32 = H + 2 * PAD;
const NC: usize = (PW * PH) as usize;
const WORDS: usize = NC.div_ceil(64);
const MAXF: usize = 48;
const MAX_AP: i8 = 10;
const SPAWN_CYCLES: u8 = 6;
const R: i32 = 40; // rank table half size
const RS: i32 = 2 * R + 1;

const DIRS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
const DIR_NAMES: [&str; 4] = ["N", "E", "S", "W"];
const DIAGS: [(i32, i32); 4] = [(1, -1), (1, 1), (-1, 1), (-1, -1)];
const DIAG_NAMES: [&str; 4] = ["NE", "SE", "SW", "NW"];
// actions: 0..4 MOVE, 4..8 JUMP, 8..12 IMMO, 12 PASS
const PASS: u8 = 12;

const RUNNING: u8 = 0;
const WON: u8 = 1;
const DEAD: u8 = 2;

fn action_name(a: u8) -> String {
    match a {
        0..=3 => format!("MOVE {}", DIR_NAMES[a as usize]),
        4..=7 => format!("JUMP {}", DIR_NAMES[a as usize - 4]),
        8..=11 => format!("IMMO {}", DIAG_NAMES[a as usize - 8]),
        _ => "PASS".to_string(),
    }
}

fn idx(x: i32, y: i32) -> usize {
    ((y + PAD) * PW + x + PAD) as usize
}

struct Ctx {
    wall: Vec<bool>,
    xs: Vec<i32>,
    ys: Vec<i32>,
    rank: Vec<u16>,
    zob: Vec<u64>,
    zpos: Vec<u64>,
    /// diagonal push blocked by: 0 nothing, 1 both corners, 2 vertical corner, 3 horizontal corner,
    /// 4 always (diagonal flames are not pushed, as on CodinGame)
    corner: u8,
    /// neighbours are pushed only when the move extinguished a flame (as on CodinGame)
    push_only_on_extinguish: bool,
}

impl Ctx {
    fn new(rows: &[String]) -> Ctx {
        let mut wall = vec![true; NC];
        for (y, r) in rows.iter().enumerate() {
            for (x, c) in r.chars().enumerate() {
                if (x as i32) < W && (y as i32) < H {
                    wall[idx(x as i32, y as i32)] = c == '#';
                }
            }
        }
        let xs = (0..NC).map(|i| i as i32 % PW - PAD).collect();
        let ys = (0..NC).map(|i| i as i32 / PW - PAD).collect();
        // attraction order: manhattan distance, then clockwise angle from north
        let mut offs = Vec::new();
        for dy in -R..=R {
            for dx in -R..=R {
                let a = (dx as f64).atan2(-dy as f64);
                let a = if a < 0.0 { a + std::f64::consts::TAU } else { a };
                offs.push((dx.abs() + dy.abs(), a, dx, dy));
            }
        }
        offs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
        let mut rank = vec![0u16; (RS * RS) as usize];
        for (r, o) in offs.iter().enumerate() {
            rank[((o.3 + R) * RS + o.2 + R) as usize] = r as u16;
        }
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let zob = (0..NC).map(|_| rnd()).collect();
        let zpos = (0..NC).map(|_| rnd()).collect();
        let corner = std::env::var("FLAME_CORNER").ok().and_then(|c| c.parse().ok()).unwrap_or(4);
        let push_only_on_extinguish = std::env::var("FLAME_PUSHX").map_or(true, |v| v != "0");
        Ctx { wall, xs, ys, rank, zob, zpos, corner, push_only_on_extinguish }
    }
}

#[derive(Clone)]
struct St {
    occ: [u64; WORDS],
    flames: [u16; MAXF],
    glyphs: [u16; 8],
    pos: u16,
    hp: i16,
    nf: u8,
    ng: u8,
    ap: i8,
    jumps: i8,
    cycle: u8,
    status: u8,
}

impl St {
    #[inline]
    fn has(&self, c: usize) -> bool {
        (self.occ[c >> 6] >> (c & 63)) & 1 != 0
    }
    #[inline]
    fn set(&mut self, c: usize) {
        self.occ[c >> 6] |= 1 << (c & 63);
    }
    #[inline]
    fn clr(&mut self, c: usize) {
        self.occ[c >> 6] &= !(1 << (c & 63));
    }

    fn add_flame(&mut self, c: usize) {
        self.flames[self.nf as usize] = c as u16;
        self.nf += 1;
        self.set(c);
    }

    fn hash(&self, ctx: &Ctx) -> u64 {
        let mut h = ctx.zpos[self.pos as usize] ^ (self.jumps as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for &f in &self.flames[..self.nf as usize] {
            h ^= ctx.zob[f as usize];
        }
        h
    }

    fn can(&self, ctx: &Ctx, a: u8) -> bool {
        match a {
            0..=3 => {
                let (dx, dy) = DIRS[a as usize];
                self.ap >= 1 && !ctx.wall[(self.pos as i32 + dx + dy * PW) as usize]
            }
            4..=7 => {
                let (dx, dy) = DIRS[a as usize - 4];
                self.ap >= 2 && self.jumps > 0 && !ctx.wall[(self.pos as i32 + 2 * (dx + dy * PW)) as usize]
            }
            8..=11 => self.ap >= 1,
            _ => true,
        }
    }

    fn apply(&mut self, ctx: &Ctx, a: u8) {
        match a {
            0..=3 => self.do_move(ctx, a as usize, 1),
            4..=7 => self.do_move(ctx, a as usize - 4, 2),
            8..=11 => {
                self.ap -= 1;
                self.hp -= 5;
                if self.hp <= 0 {
                    self.status = DEAD;
                    return;
                }
                let (dx, dy) = DIAGS[a as usize - 8];
                let t = (self.pos as i32 + dx + dy * PW) as usize;
                self.attract(ctx, t, true);
                self.check_win();
            }
            _ => self.pass(),
        }
    }

    fn check_win(&mut self) {
        if self.status == RUNNING && self.nf == 0 && self.ng == 0 && self.cycle > SPAWN_CYCLES {
            self.status = WON;
        }
    }

    fn do_move(&mut self, ctx: &Ctx, d: usize, dist: i32) {
        if self.hp <= 1 {
            self.hp = 0;
            self.status = DEAD;
            return;
        }
        self.hp -= 1;
        self.ap -= dist as i8;
        if dist == 2 {
            self.jumps -= 1;
        }
        let (dx, dy) = DIRS[d];
        let p = (self.pos as i32 + (dx + dy * PW) * dist) as usize;
        self.pos = p as u16;
        let extinguished = self.has(p);
        if extinguished {
            let n = self.nf as usize;
            let k = self.flames[..n].iter().position(|&f| f as usize == p).unwrap();
            self.flames[k] = self.flames[n - 1];
            self.nf -= 1;
            self.clr(p);
            self.hp += 1;
        }
        if !extinguished && ctx.push_only_on_extinguish {
            // nothing is pushed: an adjacent flame will be pulled onto the player
            if !self.attract(ctx, p, false) {
                self.status = DEAD;
                return;
            }
            self.check_win();
            return;
        }
        // push the 8 neighbours away
        let (px, py) = (ctx.xs[p], ctx.ys[p]);
        let mut pushed = [(0usize, 0usize); 8];
        let mut np = 0;
        for k in 0..self.nf as usize {
            let f = self.flames[k] as usize;
            let (rx, ry) = (ctx.xs[f] - px, ctx.ys[f] - py);
            if rx.abs() <= 1 && ry.abs() <= 1 {
                if self.push_blocked(ctx, f, rx, ry) {
                    self.status = DEAD;
                    return;
                }
                let dest = 2 * f - p;
                pushed[np] = (k, dest);
                np += 1;
            }
        }
        for &(k, _) in &pushed[..np] {
            self.clr(self.flames[k] as usize);
        }
        for &(k, dest) in &pushed[..np] {
            self.set(dest);
            self.flames[k] = dest as u16;
        }
        if !self.attract(ctx, p, false) {
            self.status = DEAD;
            return;
        }
        self.check_win();
    }

    fn attract(&mut self, ctx: &Ctx, t: usize, immo: bool) -> bool {
        let (tx, ty) = (ctx.xs[t], ctx.ys[t]);
        let n = self.nf as usize;
        let mut keys = [0u32; MAXF];
        for k in 0..n {
            let f = self.flames[k] as usize;
            let (dx, dy) = (ctx.xs[f] - tx, ctx.ys[f] - ty);
            keys[k] = (ctx.rank[((dy + R) * RS + dx + R) as usize] as u32) << 8 | k as u32;
        }
        keys[..n].sort_unstable();
        for &key in &keys[..n] {
            let k = (key & 0xff) as usize;
            let f = self.flames[k] as usize;
            let (ddx, ddy) = (tx - ctx.xs[f], ty - ctx.ys[f]);
            let (ax, ay) = (ddx.abs(), ddy.abs());
            if ax == 0 && ay == 0 {
                continue;
            }
            let (sx, sy) = if ax == 0 {
                (0, ddy.signum())
            } else if ay == 0 || ax > ay {
                (ddx.signum(), 0)
            } else if ax == ay {
                (ddx.signum(), ddy.signum())
            } else {
                (0, ddy.signum())
            };
            let dest = (f as i32 + sx + sy * PW) as usize;
            if ctx.wall[dest] || self.has(dest) {
                continue;
            }
            if dest == self.pos as usize {
                if immo {
                    continue;
                }
                return false;
            }
            self.clr(f);
            self.set(dest);
            self.flames[k] = dest as u16;
        }
        true
    }

    fn pass(&mut self) {
        for i in 0..self.ng as usize {
            let g = self.glyphs[i] as usize;
            if g != self.pos as usize && !self.has(g) {
                self.add_flame(g);
            }
        }
        self.ng = 0;
        self.ap = MAX_AP;
        self.jumps = 2;
        self.cycle += 1;
        if self.cycle > SPAWN_CYCLES {
            if self.nf == 0 {
                self.status = WON;
                return;
            }
            self.hp -= 1;
            if self.hp <= 0 {
                self.status = DEAD;
            }
        }
        // cycle <= 6: unknown glyphs spawn (no HP loss)
    }

    /// Predicted final HP (heuristic).
    fn eval(&self, ctx: &Ctx, prm: &Params) -> f32 {
        match self.status {
            DEAD => return -1000.0,
            WON => return self.hp as f32 + 0.5,
            _ => {}
        }
        let (px, py) = (ctx.xs[self.pos as usize], ctx.ys[self.pos as usize]);
        let mut c = [0i32; MAXF + 8];
        let mut n = 0;
        let mut sum = 0;
        let cells = self.flames[..self.nf as usize].iter().chain(&self.glyphs[..self.ng as usize]);
        let mut near = 0;
        for &f in cells {
            let d = (ctx.xs[f as usize] - px).abs().max((ctx.ys[f as usize] - py).abs());
            near += (d <= 2) as i32;
            c[n] = d;
            sum += d;
            n += 1;
        }
        c[..n].sort_unstable();
        let mut waste = 0;
        for (i, &d) in c[..n].iter().enumerate() {
            waste = waste.max(d - 1 - i as i32);
        }
        let waste = waste as f32 * prm.waste;
        let mut v = self.hp as f32 - waste - prm.dist * sum as f32;
        // crowds around the player are deadly: only cardinal flames can be stepped on
        v -= prm.crowd * (near - 2).max(0) as f32;
        // every remaining flame costs something: otherwise passing looks free
        v -= if self.cycle >= SPAWN_CYCLES { prm.count } else { prm.mcount } * n as f32;
        if self.nf > 0 {
            let (safe, free) = self.safe_moves(ctx);
            if safe == 0 {
                v -= prm.stuck;
            } else {
                v += prm.mobility * (safe.min(4) as f32) + prm.free * (free.min(2) as f32);
            }
        }
        if self.cycle >= SPAWN_CYCLES {
            let needed = n as f32 + waste;
            let extra = if self.cycle > SPAWN_CYCLES {
                ((needed - self.ap as f32).max(0.0) / 10.0).ceil()
            } else {
                1.0 + ((needed - self.ap as f32 - 10.0).max(0.0) / 10.0).ceil()
            };
            v -= extra;
        }
        v
    }

    /// Would the flame on `f`, at offset (rx, ry) from the player, be blocked when pushed?
    #[inline]
    fn push_blocked(&self, ctx: &Ctx, f: usize, rx: i32, ry: i32) -> bool {
        let free = |c: usize| !ctx.wall[c] && !self.has(c);
        let dest = (f as i32 + rx + ry * PW) as usize;
        if !free(dest) {
            return true;
        }
        if rx == 0 || ry == 0 {
            return false;
        }
        // diagonal push: see Ctx::corner
        let (cx, cy) = ((f as i32 + rx) as usize, (f as i32 + ry * PW) as usize);
        match ctx.corner {
            0 => false,
            1 => !free(cx) || !free(cy),
            2 => !free(cy),
            3 => !free(cx),
            // CodinGame: diagonal flames are not pushed, the attraction then pulls them onto the player
            _ => true,
        }
    }

    /// Number of MOVE/JUMP landing cells (AP ignored) that would not kill, and how many of
    /// them extinguish a flame.
    fn safe_moves(&self, ctx: &Ctx) -> (u32, u32) {
        let mut n = 0;
        let mut free = 0;
        for (dx, dy) in DIRS {
            for dist in 1..=2 {
                let p = self.pos as i32 + (dx + dy * PW) * dist;
                if ctx.wall[p as usize] {
                    continue;
                }
                let mut ok = true;
                let pushes = self.has(p as usize) || !ctx.push_only_on_extinguish;
                'n: for ry in -1..=1 {
                    for rx in -1..=1 {
                        let f = (p + rx + ry * PW) as usize;
                        if (rx != 0 || ry != 0) && self.has(f) && (!pushes || self.push_blocked(ctx, f, rx, ry)) {
                            ok = false;
                            break 'n;
                        }
                    }
                }
                n += ok as u32;
                free += (ok && self.has(p as usize)) as u32;
            }
        }
        (n, free)
    }

    fn has_safe_move(&self, ctx: &Ctx) -> bool {
        let mut s = self.clone();
        if s.ap < 2 {
            s.ap = MAX_AP;
            s.jumps = 2;
        }
        (0..8u8).any(|a| {
            if !s.can(ctx, a) {
                return false;
            }
            let mut c = s.clone();
            c.apply(ctx, a);
            c.status != DEAD
        })
    }
}

struct Params {
    waste: f32,
    dist: f32,
    count: f32,
    stuck: f32,
    mcount: f32,
    free: f32,
    pass: f32,
    crowd: f32,
    mobility: f32,
}

struct Node {
    st: St,
    id: u32,
    ev: f32,
    h: u64,
}

struct Searcher {
    /// beam width, separately for mid-game [0] and endgame [1] (different horizons)
    widths: [usize; 2],
    plan: Vec<u8>,
    plan_horizon: u8,
}

fn plan_value(ctx: &Ctx, prm: &Params, root: &St, plan: &[u8], horizon: u8) -> Option<f32> {
    let mut s = root.clone();
    for &a in plan {
        if !s.can(ctx, a) {
            return None;
        }
        if a == PASS && s.cycle >= horizon {
            return Some(terminal_value(ctx, prm, &s));
        }
        s.apply(ctx, a);
        match s.status {
            DEAD => return None,
            WON => return Some(s.eval(ctx, prm)),
            _ => {}
        }
    }
    None
}

/// Value of a node that PASSes at the end of the horizon (state before the PASS).
fn terminal_value(ctx: &Ctx, prm: &Params, s: &St) -> f32 {
    if s.cycle >= SPAWN_CYCLES {
        let mut c = s.clone();
        c.pass();
        c.eval(ctx, prm)
    } else {
        s.eval(ctx, prm)
    }
}

impl Searcher {
    fn horizon(root: &St) -> u8 {
        if root.cycle < SPAWN_CYCLES { root.cycle + 1 } else { root.cycle.max(SPAWN_CYCLES + 1) + 2 }
    }

    fn search(&mut self, ctx: &Ctx, prm: &Params, root: &St, start: Instant, budget_ms: f64) -> u8 {
        let horizon = Self::horizon(root);
        let phase = (root.cycle >= SPAWN_CYCLES) as usize;
        let width = self.widths[phase];
        let slot = |s: &St| (s.cycle - root.cycle) as usize * 11 + (MAX_AP - s.ap) as usize;
        let nb = (horizon - root.cycle + 1) as usize * 11;
        let mut buckets: Vec<Vec<Node>> = (0..nb).map(|_| Vec::new()).collect();
        let mut arena: Vec<(u32, u8)> = vec![(u32::MAX, PASS)];
        let mut terms: Vec<(f32, u32, St)> = Vec::new();
        buckets[slot(root)].push(Node { st: root.clone(), id: 0, ev: 0.0, h: 0 });
        let mut timed_out = false;
        let mut fallback: Option<(f32, u32)> = None;

        for b in 0..nb {
            let mut nodes = std::mem::take(&mut buckets[b]);
            if nodes.is_empty() || timed_out {
                continue;
            }
            nodes.sort_unstable_by(|a, b| b.ev.partial_cmp(&a.ev).unwrap());
            let mut seen = std::collections::HashSet::with_capacity(width * 2);
            nodes.retain(|n| seen.insert(n.h));
            nodes.truncate(width);
            for node in &nodes {
                if start.elapsed().as_secs_f64() * 1000.0 > budget_ms {
                    timed_out = true;
                    if fallback.is_none_or(|f| f.0 < node.ev) {
                        fallback = Some((node.ev, node.id));
                    }
                    break;
                }
                let mut alive = 0;
                for a in (0..8u8).chain([PASS]) {
                    if !node.st.can(ctx, a) {
                        continue;
                    }
                    if a == PASS && node.st.cycle >= horizon {
                        arena.push((node.id, a));
                        let v = terminal_value(ctx, prm, &node.st);
                        terms.push((v, arena.len() as u32 - 1, node.st.clone()));
                        continue;
                    }
                    let mut c = node.st.clone();
                    c.apply(ctx, a);
                    if c.status == DEAD {
                        continue;
                    }
                    if a != PASS {
                        alive += 1;
                    }
                    arena.push((node.id, a));
                    let id = arena.len() as u32 - 1;
                    let ev = c.eval(ctx, prm);
                    if c.status == WON {
                        terms.push((ev, id, c));
                        continue;
                    }
                    let h = c.hash(ctx) ^ ((c.cycle as u64) << 60);
                    let sl = slot(&c);
                    buckets[sl].push(Node { st: c, id, ev, h });
                }
                if alive <= 1 {
                    // (almost) stuck: allow IMMO
                    for a in 8..12u8 {
                        if !node.st.can(ctx, a) {
                            continue;
                        }
                        let mut c = node.st.clone();
                        c.apply(ctx, a);
                        if c.status == DEAD {
                            continue;
                        }
                        arena.push((node.id, a));
                        let id = arena.len() as u32 - 1;
                        let ev = c.eval(ctx, prm);
                        if c.status == WON {
                            terms.push((ev, id, c));
                            continue;
                        }
                        let h = c.hash(ctx) ^ ((c.cycle as u64) << 60);
                        let sl = slot(&c);
                        buckets[sl].push(Node { st: c, id, ev, h });
                    }
                }
            }
        }

        // safety check of the best terminals
        terms.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        // endgame: flames only move when we act, so passing with AP left is wasted time
        let pass_pen = if root.cycle >= SPAWN_CYCLES { prm.pass * root.ap as f32 / 10.0 } else { 0.0 };
        let first_action = |mut i: u32| {
            while arena[arena[i as usize].0 as usize].0 != u32::MAX {
                i = arena[i as usize].0;
            }
            arena[i as usize].1
        };
        let mut best: Option<(f32, u32)> = None;
        for (i, (v, id, s)) in terms.iter().take(300).enumerate() {
            let mut v = *v;
            if i < 30 && s.status == RUNNING && !s.has_safe_move(ctx) {
                v -= 6.0;
            }
            if pass_pen > 0.0 && first_action(*id) == PASS {
                v -= pass_pen;
            }
            if best.is_none_or(|b| b.0 < v) {
                best = Some((v, *id));
            }
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if timed_out {
            self.widths[phase] = (width / 2).max(10);
        } else if elapsed < budget_ms * 0.5 {
            self.widths[phase] = (width * 5 / 4 + 1).min(20000);
        }
        let mut best = best.or(fallback);
        // previous plan still valid and better?
        if !self.plan.is_empty() && self.plan_horizon == horizon {
            if let Some(pv) = plan_value(ctx, prm, root, &self.plan, horizon) {
                let pv = if self.plan[0] == PASS { pv - pass_pen } else { pv };
                if best.is_none_or(|b| b.0 < pv - 1e-4) {
                    eprintln!("keep plan {pv:.2} vs {:?}", best.map(|b| b.0));
                    let a = self.plan.remove(0);
                    return a;
                }
            }
        }
        let Some((v, id)) = best.take() else {
            self.plan.clear();
            return PASS;
        };
        let mut seq = Vec::new();
        let mut i = id;
        while arena[i as usize].0 != u32::MAX {
            seq.push(arena[i as usize].1);
            i = arena[i as usize].0;
        }
        seq.reverse();
        eprintln!(
            "cycle {} hp {} ap {} ev {v:.2} width {} terms {} t {elapsed:.1}ms plan {:?}",
            root.cycle,
            root.hp,
            root.ap,
            width,
            terms.len(),
            seq.iter().map(|&a| action_name(a)).collect::<Vec<_>>().join(",")
        );
        let a = seq.first().copied().unwrap_or(PASS);
        self.plan = seq.into_iter().skip(1).collect();
        self.plan_horizon = horizon;
        a
    }
}

fn main() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map(|l| l.unwrap());
    let mut next = || lines.next().expect("eof");
    let first = next();
    let mut it = first.split_whitespace();
    let height: usize = it.next().unwrap().parse().unwrap();
    let rows: Vec<String> = (0..height).map(|_| next()).collect();
    let ctx = Ctx::new(&rows);
    let mut prm = Params { waste: 0.3, dist: 0.01, count: 0.6, stuck: 15.0, mobility: 0.1, mcount: 0.8, free: 0.3, pass: 1.0, crowd: 0.3 };
    // tuning hook: FLAME_PARAMS="waste,dist,count,stuck,mobility,mcount,free,pass,crowd"
    if let Ok(p) = std::env::var("FLAME_PARAMS") {
        let v: Vec<f32> = p.split(',').map(|x| x.parse().unwrap()).collect();
        prm = Params { waste: v[0], dist: v[1], count: v[2], stuck: v[3], mobility: v[4], mcount: v[5], free: v[6], pass: v[7], crowd: v[8] };
    }
    let mut searcher = Searcher { widths: [200, 100], plan: Vec::new(), plan_horizon: 0 };
    let budget_ms: f64 = std::env::var("FLAME_BUDGET").ok().and_then(|b| b.parse().ok()).unwrap_or(55.0);
    // debugging hook: start from a later cycle (to replay a dumped STATE)
    let mut cycle: u8 = std::env::var("FLAME_START_CYCLE").ok().and_then(|c| c.parse().ok()).unwrap_or(1);
    let mut turn = 0;
    let mut predicted: Option<St> = None;

    loop {
        let line = next();
        let start = Instant::now();
        turn += 1;
        let v: Vec<i32> = line.split_whitespace().map(|x| x.parse().unwrap()).collect();
        let pos = idx(v[0], v[1]);
        let v: Vec<i32> = next().split_whitespace().map(|x| x.parse().unwrap()).collect();
        let (hp, ap, jumps) = (v[0], v[1], v[2]);
        let mut st = St {
            occ: [0; WORDS],
            flames: [0; MAXF],
            glyphs: [0; 8],
            pos: pos as u16,
            hp: hp as i16,
            nf: 0,
            ng: 0,
            ap: ap as i8,
            jumps: jumps as i8,
            cycle,
            status: RUNNING,
        };
        let nf: usize = next().trim().parse().unwrap();
        for _ in 0..nf {
            let v: Vec<i32> = next().split_whitespace().map(|x| x.parse().unwrap()).collect();
            st.add_flame(idx(v[0], v[1]));
        }
        let ng: usize = next().trim().parse().unwrap();
        for i in 0..ng {
            let v: Vec<i32> = next().split_whitespace().map(|x| x.parse().unwrap()).collect();
            st.glyphs[i] = idx(v[0], v[1]) as u16;
        }
        st.ng = ng as u8;
        let na: usize = next().trim().parse().unwrap();
        for _ in 0..na {
            next();
        }

        let budget = if turn == 1 { 600.0 } else { budget_ms };
        if let Some(p) = &predicted {
            let mut a: Vec<u16> = p.flames[..p.nf as usize].to_vec();
            let mut b: Vec<u16> = st.flames[..st.nf as usize].to_vec();
            a.sort_unstable();
            b.sort_unstable();
            if a != b || p.pos != st.pos || p.hp != st.hp || p.ap != st.ap || p.jumps != st.jumps {
                eprintln!("DESYNC turn {turn}");
            }
        }
        // compact dump of the turn input, to reproduce CodinGame games locally
        let xy = |c: &u16| format!("{},{}", ctx.xs[*c as usize], ctx.ys[*c as usize]);
        eprintln!(
            "STATE c{cycle} pos {} hp {hp} ap {ap} j {jumps} | F {} | G {}",
            xy(&st.pos),
            st.flames[..st.nf as usize].iter().map(xy).collect::<Vec<_>>().join(" "),
            st.glyphs[..st.ng as usize].iter().map(xy).collect::<Vec<_>>().join(" ")
        );
        let a = searcher.search(&ctx, &prm, &st, start, budget);
        let mut p = st.clone();
        p.apply(&ctx, a);
        predicted = Some(p);
        if a == PASS {
            cycle += 1;
        }
        println!("{}", action_name(a));
    }
}
