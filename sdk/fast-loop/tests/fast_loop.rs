//! Fast-loop qualification on a fixture map.
//!
//! The harness is a transactional test host (file overlay, recorded calls, rollback on
//! a failed step), a deterministic world, and a mock LLM that answers after a fixed
//! number of ticks. Nothing here is the native runtime or a real engine; replay means
//! re-executing the guest policy against the recorded batches and inbox bytes.
use coglet_fast_loop::{Budget, Command, Config, Controller, Exhausted, FastLoop, State};
use coglet_guest_rust::{Context, Event, Host, HostError, Operation};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const COGLET: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const ROUTER: &str = "hub:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const SURFACE: &str = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const STATE: &str = "/data/fast-loop.json";
const TICK_US: u64 = 20_000;

/// Stated budgets for the qualification (see README).
const WORK_BUDGET: u64 = 64;
const WALL_BUDGET: Duration = Duration::from_millis(50);

// ---- fixture map and world -------------------------------------------------------

const MAP: [&str; 9] = [
    "################",
    "#..............#",
    "#.......#......#",
    "#.......#......#",
    "#.S.....#....G.#",
    "#.......#......#",
    "#.......#......#",
    "#..............#",
    "################",
];

#[derive(Clone)]
struct World {
    pos: (i32, i32),
    goal: (i32, i32),
    moves: u64,
}
fn open(x: i32, y: i32) -> bool {
    MAP.get(y as usize)
        .and_then(|row| row.as_bytes().get(x as usize))
        .is_some_and(|c| *c != b'#')
}
impl World {
    fn new() -> Self {
        let find = |c: u8| {
            for (y, row) in MAP.iter().enumerate() {
                if let Some(x) = row.bytes().position(|b| b == c) {
                    return (x as i32, y as i32);
                }
            }
            unreachable!()
        };
        Self {
            pos: find(b'S'),
            goal: find(b'G'),
            moves: 0,
        }
    }
    fn obs(&self) -> Value {
        let (x, y) = self.pos;
        json!({"x": x, "y": y, "gx": self.goal.0, "gy": self.goal.1,
               "open": [open(x, y - 1), open(x + 1, y), open(x, y + 1), open(x - 1, y)],
               "done": self.pos == self.goal})
    }
    fn apply(&mut self, action: &Value) {
        assert_eq!(action["capability"], "fixture.move");
        let (dx, dy) = match action["args"]["dir"].as_str().unwrap() {
            "n" => (0, -1),
            "e" => (1, 0),
            "s" => (0, 1),
            "w" => (-1, 0),
            other => panic!("bad dir {other}"),
        };
        if open(self.pos.0 + dx, self.pos.1 + dy) {
            self.pos = (self.pos.0 + dx, self.pos.1 + dy);
        }
        self.moves += 1;
    }
}

// ---- mock LLM: routes around obstacles; deterministic function of the request -----

fn route(from: (i32, i32), goal: (i32, i32)) -> Vec<(i32, i32)> {
    let mut parent: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
    let mut queue = VecDeque::from([from]);
    parent.insert(from, from);
    while let Some(p) = queue.pop_front() {
        if p == goal {
            break;
        }
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let n = (p.0 + dx, p.1 + dy);
            if open(n.0, n.1) && !parent.contains_key(&n) {
                parent.insert(n, p);
                queue.push_back(n);
            }
        }
    }
    let mut cells = vec![goal];
    while *cells.last().unwrap() != from {
        cells.push(parent[cells.last().unwrap()]);
    }
    cells.reverse();
    // Keep corners only: the controller walks straight segments between waypoints.
    let mut out = vec![];
    for i in 1..cells.len() {
        let last = i + 1 == cells.len();
        let turn = !last
            && (cells[i].0 - cells[i - 1].0, cells[i].1 - cells[i - 1].1)
                != (cells[i + 1].0 - cells[i].0, cells[i + 1].1 - cells[i].1);
        if last || turn {
            out.push(cells[i]);
        }
    }
    out
}

// ---- the fixture controller -------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Waypoints {
    cells: Vec<(i32, i32)>,
    cursor: usize,
}
struct Obs {
    pos: (i32, i32),
    goal: (i32, i32),
    open: [bool; 4],
    done: bool,
}
struct Avatar;

fn step_toward(obs: &Obs, target: (i32, i32)) -> Option<Command> {
    let (dx, dy) = (target.0 - obs.pos.0, target.1 - obs.pos.1);
    let horizontal = (if dx > 0 { "e" } else { "w" }, if dx > 0 { 1 } else { 3 });
    let vertical = (if dy > 0 { "s" } else { "n" }, if dy > 0 { 2 } else { 0 });
    let candidates = if dx.abs() >= dy.abs() {
        [(dx, horizontal), (dy, vertical)]
    } else {
        [(dy, vertical), (dx, horizontal)]
    };
    for (delta, (dir, side)) in candidates {
        if delta != 0 && obs.open[side] {
            return Some(Command {
                capability: "fixture.move".into(),
                args: Map::from_iter([("dir".into(), json!(dir))]),
            });
        }
    }
    None
}

impl Controller for Avatar {
    type Plan = Waypoints;
    type Obs = Obs;
    fn observation_kind(&self) -> &str {
        "fixture.state"
    }
    fn parse_observation(&self, p: &Value) -> Option<Obs> {
        let open = p["open"].as_array()?;
        Some(Obs {
            pos: (p["x"].as_i64()? as i32, p["y"].as_i64()? as i32),
            goal: (p["gx"].as_i64()? as i32, p["gy"].as_i64()? as i32),
            open: [
                open.first()?.as_bool()?,
                open.get(1)?.as_bool()?,
                open.get(2)?.as_bool()?,
                open.get(3)?.as_bool()?,
            ],
            done: p["done"].as_bool()?,
        })
    }
    fn is_terminal(&self, obs: &Obs) -> bool {
        obs.done
    }
    fn decide(
        &self,
        obs: &Obs,
        plan: Option<&mut Waypoints>,
        budget: &mut Budget,
    ) -> Result<Option<Command>, Exhausted> {
        budget.charge(2)?;
        let mut target = obs.goal;
        if let Some(plan) = plan {
            // Skip waypoints already reached; each skip is one charged unit.
            while plan.cursor < plan.cells.len() && plan.cells[plan.cursor] == obs.pos {
                budget.charge(1)?;
                plan.cursor += 1;
            }
            if let Some(cell) = plan.cells.get(plan.cursor) {
                target = *cell;
            }
        }
        budget.charge(1)?;
        Ok(step_toward(obs, target))
    }
    fn fallback(&self, obs: &Obs) -> Option<Command> {
        step_toward(obs, obs.goal)
    }
    fn wants_plan(&self, _: &Obs, plan: Option<&Waypoints>) -> bool {
        plan.is_none()
    }
    fn plan_request(&self, obs: &Obs) -> Map<String, Value> {
        Map::from_iter([
            ("from".into(), json!([obs.pos.0, obs.pos.1])),
            ("goal".into(), json!([obs.goal.0, obs.goal.1])),
        ])
    }
    fn parse_plan(&self, output: &Value) -> Option<Waypoints> {
        let list = output["waypoints"].as_array()?;
        if list.is_empty() || list.len() > 16 {
            return None;
        }
        let cells = list
            .iter()
            .map(|c| {
                Some((
                    i32::try_from(c.get(0)?.as_i64()?).ok()?,
                    i32::try_from(c.get(1)?.as_i64()?).ok()?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        cells
            .iter()
            .all(|c| open(c.0, c.1))
            .then_some(Waypoints { cells, cursor: 0 })
    }
    fn plan_finished(&self, _: &Obs, plan: &Waypoints) -> bool {
        plan.cursor >= plan.cells.len()
    }
}

fn config() -> Config {
    Config {
        coglet: COGLET.into(),
        llm_sender: ROUTER.into(),
        route: "planner".into(),
        tick_budget: WORK_BUDGET,
        plan_ttl_ticks: 400,
        plan_timeout_ticks: 60,
        backoff_ticks: 10,
        action_deadline_us: 10_000_000,
        state_path: STATE.into(),
    }
}

// ---- transactional test host and recorded steps -----------------------------------

#[derive(Default, Clone)]
struct SimHost {
    files: BTreeMap<String, Vec<u8>>,
    calls: Vec<(Operation, Vec<u8>)>,
    reads: Vec<String>,
    refuse_llm: bool,
}
impl Host for SimHost {
    fn call(&mut self, op: Operation, bytes: &[u8]) -> i32 {
        if op == Operation::Llm && self.refuse_llm {
            return 3;
        }
        self.calls.push((op, bytes.to_vec()));
        0
    }
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
        self.reads.push(path.into());
        Ok(self.files.get(path).cloned())
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
        self.files.insert(path.into(), bytes.to_vec());
        Ok(())
    }
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the runtime would record for one committed step.
#[derive(Clone)]
struct Recorded {
    step: u64,
    time: u64,
    seed: u64,
    events: Vec<Value>,
    inbox_appended: Vec<u8>,
    calls: Vec<(Operation, Vec<u8>)>,
    files_hash: String,
    wall: Duration,
}
struct Out {
    actions: Vec<Value>,
    requests: Vec<Value>,
}

struct Rig {
    host: SimHost,
    fl: FastLoop<Avatar>,
    step: u64,
    inbox_next: u64,
    seq: u64,
    journal: Vec<Recorded>,
}
fn ev(source: &str, kind: &str, seq: u64, time: u64, payload: Value, corr: &str) -> Value {
    json!({"source": source, "kind": kind, "seq": seq, "logical_time": time, "payload": payload, "correlation_id": corr})
}
fn corr(n: u64) -> String {
    format!("{n:032x}")
}
fn files_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut h = Sha256::new();
    for (k, v) in files {
        h.update(k.as_bytes());
        h.update(sha(v).as_bytes());
    }
    format!("{:x}", h.finalize())
}
fn decode(op: Operation, bytes: &[u8]) -> Value {
    let rule = match op {
        Operation::Emit => "common::action",
        Operation::Llm => "abi::llm-request",
        _ => panic!("unexpected op"),
    };
    coglet_types::decode_typed(rule, bytes).unwrap()
}

impl Rig {
    fn new(config: Config) -> Self {
        Self {
            host: SimHost::default(),
            fl: FastLoop::new(Avatar, config),
            step: 0,
            inbox_next: 0,
            seq: 0,
            journal: vec![],
        }
    }
    fn state(&self) -> State<Waypoints> {
        serde_json::from_slice(&self.host.files[STATE]).unwrap()
    }
    /// Append inbox lines (sender, kind, payload) and run one step with their wakes plus
    /// optional observation. A failing step rolls back files and effects and is not recorded.
    fn step(
        &mut self,
        appends: &[(&str, &str, Value)],
        obs: Option<Value>,
    ) -> Result<Out, HostError> {
        let time = (self.step + 1) * TICK_US;
        let mut events = vec![];
        let mut inbox = vec![];
        let mut next = self.inbox_next;
        for (sender, kind, payload) in appends {
            self.seq += 1;
            let id = corr(0x1000 + next);
            let inner = ev(sender, kind, self.seq, time, payload.clone(), &id);
            let row = json!({"event": inner, "id": id, "index": next, "sender": sender});
            inbox.extend_from_slice(serde_json::to_string(&row).unwrap().as_bytes());
            inbox.push(b'\n');
            events.push(ev(
                COGLET,
                "runtime.inbox",
                self.seq,
                time,
                json!({"index": next, "id": id}),
                &id,
            ));
            next += 1;
        }
        if let Some(obs) = obs {
            self.seq += 1;
            events.push(ev(
                SURFACE,
                "fixture.state",
                self.seq,
                time,
                obs,
                &corr(0x2000 + self.step),
            ));
        }
        let seed = 42;
        self.run(self.step, time, seed, events, inbox, true)?;
        self.inbox_next = next;
        let step = self.journal.last().unwrap();
        let mut out = Out {
            actions: vec![],
            requests: vec![],
        };
        for (op, bytes) in &step.calls {
            match op {
                Operation::Emit => out.actions.push(decode(*op, bytes)),
                Operation::Llm => out.requests.push(decode(*op, bytes)),
                _ => {}
            }
        }
        self.step += 1;
        Ok(out)
    }
    /// Execute one step against `self.host` with the given recorded inputs.
    fn run(
        &mut self,
        step: u64,
        time: u64,
        seed: u64,
        events: Vec<Value>,
        inbox: Vec<u8>,
        record: bool,
    ) -> Result<(), HostError> {
        let snapshot = self.host.clone();
        self.host.calls.clear();
        self.host.reads.clear();
        if !inbox.is_empty() {
            self.host
                .files
                .entry("/data/inbox.jsonl".into())
                .or_default()
                .extend_from_slice(&inbox);
        }
        let batch: Vec<Event> = events
            .iter()
            .map(|e| serde_json::from_value(e.clone()).unwrap())
            .collect();
        let started = Instant::now();
        let result = self
            .fl
            .wake(&mut Context::new(&mut self.host, step, time, seed), &batch);
        let wall = started.elapsed();
        if let Err(e) = result {
            self.host = SimHost {
                refuse_llm: snapshot.refuse_llm,
                files: snapshot.files,
                ..Default::default()
            };
            return Err(e);
        }
        if record {
            self.journal.push(Recorded {
                step,
                time,
                seed,
                events,
                inbox_appended: inbox,
                calls: self.host.calls.clone(),
                files_hash: files_hash(&self.host.files),
                wall,
            });
        }
        Ok(())
    }
}

// ---- closed-loop episode with an asynchronous mock LLM -----------------------------

struct Episode {
    rig: Rig,
    world: World,
    /// (deliver_at_step, request id, reply output)
    due: Vec<(u64, String, Value)>,
    trail: Vec<(i32, i32)>,
}
fn episode(config: Config, latency: u64, refuse_llm: bool, max_steps: u64) -> Episode {
    let mut rig = Rig::new(config);
    rig.host.refuse_llm = refuse_llm;
    let mut world = World::new();
    let mut due: Vec<(u64, String, Value)> = vec![];
    let mut trail = vec![world.pos];
    for _ in 0..max_steps {
        let now = rig.step;
        let ready: Vec<_> = due.iter().filter(|d| d.0 <= now).cloned().collect();
        due.retain(|d| d.0 > now);
        let appends: Vec<(&str, &str, Value)> = ready
            .iter()
            .map(|(_, id, out)| {
                (
                    ROUTER,
                    "llm.reply",
                    json!({"request_id": id, "status": "succeeded", "output": out}),
                )
            })
            .collect();
        let out = rig.step(&appends, Some(world.obs())).unwrap();
        for request in out.requests {
            let from = request["input"]["from"].as_array().unwrap();
            let goal = request["input"]["goal"].as_array().unwrap();
            let plan = route(
                (
                    from[0].as_i64().unwrap() as i32,
                    from[1].as_i64().unwrap() as i32,
                ),
                (
                    goal[0].as_i64().unwrap() as i32,
                    goal[1].as_i64().unwrap() as i32,
                ),
            );
            let cells: Vec<Value> = plan.iter().map(|c| json!([c.0, c.1])).collect();
            due.push((
                now + latency,
                request["id"].as_str().unwrap().to_string(),
                json!({"waypoints": cells}),
            ));
        }
        for action in &out.actions {
            world.apply(action);
        }
        trail.push(world.pos);
        if world.pos == world.goal {
            // One more observation lets the loop see the terminal state.
            rig.step(&[], Some(world.obs())).unwrap();
            break;
        }
    }
    Episode {
        rig,
        world,
        due,
        trail,
    }
}

// ---- tests ------------------------------------------------------------------------

#[test]
fn avatar_meets_tick_budget_while_planning_continues_asynchronously() {
    let latency = 12;
    let e = episode(config(), latency, false, 200);
    assert_eq!(
        e.world.pos, e.world.goal,
        "avatar reaches the goal; trail {:?}",
        e.trail
    );
    let state = e.rig.state();
    assert!(state.done);

    // Deterministic budget: no tick charged more than the stated work limit.
    assert!(
        state.stats.max_work <= WORK_BUDGET,
        "max work {}",
        state.stats.max_work
    );
    assert_eq!(state.stats.degraded, 0);
    // Host-measured wall budget per step (not part of replay state).
    let worst = e.rig.journal.iter().map(|r| r.wall).max().unwrap();
    assert!(worst < WALL_BUDGET, "slowest step {worst:?}");

    // The request is staged on the first tick and the plan lands exactly `latency` steps
    // later; every tick in between still acts locally instead of waiting.
    let requested = e
        .rig
        .journal
        .iter()
        .position(|r| r.calls.iter().any(|c| c.0 == Operation::Llm))
        .unwrap();
    assert_eq!(requested, 0);
    let applied = e
        .rig
        .journal
        .iter()
        .position(|r| !r.inbox_appended.is_empty())
        .unwrap();
    assert_eq!(applied as u64, requested as u64 + latency);
    for r in &e.rig.journal[requested..applied] {
        assert!(r.inbox_appended.is_empty());
    }
    let acting_while_waiting = e.rig.journal[requested..applied]
        .iter()
        .filter(|r| r.calls.iter().any(|c| c.0 == Operation::Emit))
        .count();
    assert!(
        acting_while_waiting >= 5,
        "{acting_while_waiting} acting ticks before the plan arrived"
    );
    println!(
        "fast-loop fixture: {} steps, {} moves, max work {}/{} units, slowest step {:?} (budget {:?}), \
         {} acting ticks while the plan was in flight",
        e.rig.journal.len(),
        e.world.moves,
        state.stats.max_work,
        WORK_BUDGET,
        worst,
        WALL_BUDGET,
        acting_while_waiting
    );
    // The inbox is read only when a wake names it.
    assert_eq!(state.stats.plans_applied, 1);
    assert_eq!(state.stats.plan_requests, 1);
    assert!(e.world.moves < 80, "moves {}", e.world.moves);
}

#[test]
fn reflex_alone_stalls_at_the_wall_and_the_async_plan_is_what_gets_around_it() {
    // The host refuses every LLM request: only the fast path runs.
    let reflex = episode(config(), 1, true, 120);
    assert_ne!(reflex.world.pos, reflex.world.goal);
    assert_eq!(reflex.world.pos, (7, 4), "greedy descent ends at the wall");
    let state = reflex.rig.state();
    assert!(state.stats.plan_refused > 0);
    assert!(!state.done);
    assert!(state.stats.max_work <= WORK_BUDGET);

    // Same map with a slow planner (40 ticks) still arrives.
    let slow = episode(config(), 40, false, 200);
    assert_eq!(slow.world.pos, slow.world.goal, "{:?}", slow.trail);
}

#[test]
fn replay_reexecutes_each_step_without_llm_or_world_and_matches_every_effect_and_state() {
    let live = episode(config(), 12, false, 200);
    assert!(live.due.is_empty());
    let mut replay = Rig::new(config());
    for r in &live.rig.journal {
        replay
            .run(
                r.step,
                r.time,
                r.seed,
                r.events.clone(),
                r.inbox_appended.clone(),
                true,
            )
            .unwrap();
        let again = replay.journal.last().unwrap();
        assert_eq!(again.calls, r.calls, "effects at step {}", r.step);
        assert_eq!(again.files_hash, r.files_hash, "state at step {}", r.step);
    }
    assert_eq!(replay.host.files, live.rig.host.files);
    assert_eq!(
        replay.state().digest().unwrap(),
        live.rig.state().digest().unwrap()
    );
    // Negative controls: a changed reply byte or seed must be detected.
    let mut tampered = Rig::new(config());
    let mut diverged = false;
    for r in &live.rig.journal {
        let inbox = if r.inbox_appended.is_empty() {
            vec![]
        } else {
            String::from_utf8(r.inbox_appended.clone())
                .unwrap()
                .replace("\"waypoints\":[[", "\"waypoints\":[[2,4],[")
                .into_bytes()
        };
        tampered
            .run(r.step, r.time, r.seed, r.events.clone(), inbox, true)
            .unwrap();
        diverged |= tampered.journal.last().unwrap().files_hash != r.files_hash;
    }
    assert!(diverged, "tampered reply must change replay");
    let mut reseeded = Rig::new(config());
    reseeded
        .run(
            0,
            live.rig.journal[0].time,
            43,
            live.rig.journal[0].events.clone(),
            vec![],
            true,
        )
        .unwrap();
    assert_ne!(
        reseeded.journal[0].calls, live.rig.journal[0].calls,
        "seed derives action IDs"
    );
}

#[test]
fn exhausted_budget_degrades_to_the_fallback_and_never_overruns() {
    let mut cfg = config();
    cfg.tick_budget = 2; // decide needs 3 units even with no plan
    let e = episode(cfg, 3, false, 80);
    let state = e.rig.state();
    assert_eq!(
        state.stats.degraded, state.stats.ticks,
        "every tick degraded"
    );
    assert!(state.stats.max_work <= 2);
    assert!(state.stats.actions > 0, "fallback still drives the avatar");
    assert_eq!(e.world.pos, (7, 4), "fallback is the O(1) reflex only");
}

fn plan_reply(id: &str) -> Value {
    json!({"request_id": id, "status": "succeeded", "output": {"waypoints": [[7, 1], [9, 1], [9, 4], [13, 4]]}})
}
fn request_id(out: &Out) -> String {
    out.requests[0]["id"].as_str().unwrap().to_string()
}
fn obs() -> Value {
    World::new().obs()
}

#[test]
fn failed_foreign_duplicate_and_malformed_replies_never_stall_ticks() {
    let mut rig = Rig::new(config());
    let out = rig.step(&[], Some(obs())).unwrap();
    assert_eq!((out.requests.len(), out.actions.len()), (1, 1));
    let id = request_id(&out);

    // Foreign sender with the right request id: ignored; the request stays pending.
    let foreign = "hub:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let out = rig
        .step(&[(foreign, "llm.reply", plan_reply(&id))], Some(obs()))
        .unwrap();
    assert_eq!((out.requests.len(), out.actions.len()), (0, 1));
    assert_eq!(rig.state().stats.ignored, 1);
    assert!(rig.state().plan.is_none());
    // Wrong request id and wrong kind: ignored.
    rig.step(
        &[
            (ROUTER, "llm.reply", plan_reply(&corr(9))),
            (ROUTER, "other.kind", json!({})),
        ],
        Some(obs()),
    )
    .unwrap();
    assert_eq!(rig.state().stats.ignored, 3);

    // Malformed plan (off-map waypoint): rejected, backoff, ticks keep acting.
    let bad = json!({"request_id": id, "status": "succeeded", "output": {"waypoints": [[0, 0]]}});
    let out = rig
        .step(&[(ROUTER, "llm.reply", bad)], Some(obs()))
        .unwrap();
    assert_eq!(out.actions.len(), 1);
    let s = rig.state();
    assert_eq!((s.stats.plans_rejected, s.pending.is_none()), (1, true));
    let rejected_at = s.tick;
    // No new request until the backoff passes.
    let mut next_request = None;
    for _ in 0..20 {
        let out = rig.step(&[], Some(obs())).unwrap();
        assert_eq!(out.actions.len(), 1);
        if let Some(r) = out.requests.first() {
            next_request = Some((rig.state().tick, r["id"].as_str().unwrap().to_string()));
            break;
        }
    }
    let (at, id2) = next_request.expect("request after backoff");
    assert_eq!(at, rejected_at + 10);

    // Provider failure: counted, backoff, no stall.
    let failed = json!({"request_id": id2, "status": "failed", "error": {"code": "unavailable", "message": "x", "retryable": true}});
    rig.step(&[(ROUTER, "llm.reply", failed)], Some(obs()))
        .unwrap();
    assert_eq!(rig.state().stats.plan_failures, 1);
    // A good reply to a fresh request applies once; a duplicate delivery is ignored.
    let mut id3 = None;
    for _ in 0..20 {
        if let Some(r) = rig.step(&[], Some(obs())).unwrap().requests.first() {
            id3 = Some(r["id"].as_str().unwrap().to_string());
            break;
        }
    }
    let id3 = id3.unwrap();
    rig.step(&[(ROUTER, "llm.reply", plan_reply(&id3))], Some(obs()))
        .unwrap();
    assert_eq!(rig.state().stats.plans_applied, 1);
    let before = rig.state().stats.ignored;
    rig.step(&[(ROUTER, "llm.reply", plan_reply(&id3))], Some(obs()))
        .unwrap();
    assert_eq!(rig.state().stats.ignored, before + 1);
    assert_eq!(rig.state().stats.plans_applied, 1);
}

#[test]
fn unanswered_request_times_out_and_its_late_reply_is_ignored() {
    let mut rig = Rig::new(config());
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    for _ in 0..61 {
        rig.step(&[], Some(obs())).unwrap();
    }
    let s = rig.state();
    assert_eq!((s.stats.plan_timeouts, s.pending.is_none()), (1, true));
    rig.step(&[(ROUTER, "llm.reply", plan_reply(&id))], Some(obs()))
        .unwrap();
    let s = rig.state();
    assert!(s.plan.is_none(), "stale reply must not install a plan");
    assert_eq!(s.stats.ignored, 1);
}

#[test]
fn refused_request_costs_neither_the_action_nor_the_tick() {
    let mut rig = Rig::new(config());
    rig.host.refuse_llm = true;
    let out = rig.step(&[], Some(obs())).unwrap();
    assert_eq!((out.actions.len(), out.requests.len()), (1, 0));
    let s = rig.state();
    assert_eq!(
        (s.stats.plan_refused, s.pending.is_none(), s.tick),
        (1, true, 1)
    );
}

#[test]
fn plan_expires_after_ttl_and_a_finished_plan_is_dropped() {
    let mut cfg = config();
    cfg.plan_ttl_ticks = 3;
    let mut rig = Rig::new(cfg);
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    rig.step(&[(ROUTER, "llm.reply", plan_reply(&id))], Some(obs()))
        .unwrap();
    assert!(rig.state().plan.is_some());
    for _ in 0..4 {
        rig.step(&[], Some(obs())).unwrap();
    }
    assert!(rig.state().stats.plans_expired >= 1);

    // A plan whose last waypoint is already reached is spent immediately.
    let mut rig = Rig::new(config());
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    let done = json!({"request_id": id, "status": "succeeded", "output": {"waypoints": [[2, 4]]}});
    rig.step(&[(ROUTER, "llm.reply", done)], Some(obs()))
        .unwrap();
    assert!(rig.state().plan.is_none());
}

/// One step carrying an observation and a wake for `wake_index`, over raw inbox bytes
/// that replace the whole file (so damaged shapes can be injected).
fn step_with_inbox(rig: &mut Rig, raw: &[u8], wake_index: u64) -> Result<Out, HostError> {
    rig.host
        .files
        .insert("/data/inbox.jsonl".into(), raw.to_vec());
    let time = (rig.step + 1) * TICK_US;
    let events = vec![
        ev(
            COGLET,
            "runtime.inbox",
            500 + rig.step,
            time,
            json!({"index": wake_index, "id": corr(5)}),
            &corr(5),
        ),
        ev(
            SURFACE,
            "fixture.state",
            600 + rig.step,
            time,
            obs(),
            &corr(0x2000 + rig.step),
        ),
    ];
    rig.run(rig.step, time, 42, events, vec![], true)?;
    rig.step += 1;
    let step = rig.journal.last().unwrap();
    let mut out = Out {
        actions: vec![],
        requests: vec![],
    };
    for (op, bytes) in &step.calls {
        match op {
            Operation::Emit => out.actions.push(decode(*op, bytes)),
            Operation::Llm => out.requests.push(decode(*op, bytes)),
            _ => {}
        }
    }
    Ok(out)
}
/// Inbox line for `kind`; `source` is the embedded event source, `sender` the authenticated one.
fn raw_line(
    index: u64,
    kind: &str,
    sender: &str,
    source: &str,
    payload: Value,
    corr_id: &str,
) -> Vec<u8> {
    let id = corr(0x3000 + index); // runtime-allocated, distinct from the event correlation_id
    let event = ev(source, kind, index + 1, 1, payload, corr_id);
    let row = json!({"event": event, "id": id, "index": index, "sender": sender});
    let mut line = serde_json::to_string(&row).unwrap().into_bytes();
    line.push(b'\n');
    line
}
fn inbox_line(index: u64, sender: &str, source: &str, payload: Value) -> Vec<u8> {
    raw_line(
        index,
        "llm.reply",
        sender,
        source,
        payload,
        &corr(0x1000 + index),
    )
}
fn inbox_kind_line(index: u64, kind: &str, sender: &str, payload: Value, corr_id: &str) -> Vec<u8> {
    raw_line(index, kind, sender, sender, payload, corr_id)
}

#[test]
fn damaged_inbox_is_counted_and_never_stops_the_fast_path() {
    let good = |rid: &str| inbox_line(0, ROUTER, ROUTER, plan_reply(rid));
    let foreign = "hub:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    type Make = Box<dyn Fn(&str) -> (Vec<u8>, u64)>;
    let cases: Vec<(&str, Make)> = vec![
        ("wake names a missing line", Box::new(|_| (vec![], 3))),
        (
            "CRLF line ending",
            Box::new(move |r| {
                let mut l = good(r);
                l.pop();
                l.extend_from_slice(b"\r\n");
                (l, 0)
            }),
        ),
        (
            "non-canonical spacing",
            Box::new(move |r| {
                let row: Value = serde_json::from_slice(&good(r)).unwrap();
                let mut l = serde_json::to_string_pretty(&row)
                    .unwrap()
                    .replace('\n', " ")
                    .into_bytes();
                l.push(b'\n');
                (l, 0)
            }),
        ),
        (
            "action.outcome with a malformed payload",
            Box::new(|_| {
                let p = json!({"id": corr(1), "correlation_id": corr(2), "status": "succeeded"});
                (inbox_kind_line(0, "action.outcome", ROUTER, p, &corr(2)), 0)
            }),
        ),
        (
            "action.outcome whose event and payload correlation ids differ",
            Box::new(|_| {
                let p = json!({"id": corr(1), "correlation_id": corr(2), "status": "succeeded", "result": {}});
                (inbox_kind_line(0, "action.outcome", ROUTER, p, &corr(3)), 0)
            }),
        ),
        (
            "missing final LF",
            Box::new(move |r| {
                let mut l = good(r);
                l.pop();
                (l, 0)
            }),
        ),
        (
            "non-contiguous index",
            Box::new(|r| (inbox_line(1, ROUTER, ROUTER, plan_reply(r)), 0)),
        ),
        (
            "sender differs from the embedded event source",
            Box::new(move |r| (inbox_line(0, ROUTER, foreign, plan_reply(r)), 0)),
        ),
        (
            "llm-reply fails its schema",
            Box::new(|r| {
                let mut p = plan_reply(r);
                p["unexpected"] = json!(1);
                (inbox_line(0, ROUTER, ROUTER, p), 0)
            }),
        ),
        (
            "inbox beyond the size cap",
            Box::new(|r| {
                // One otherwise valid reply padded past the cap: only the cap rejects it.
                let mut p = plan_reply(r);
                p["output"]["pad"] = json!("x".repeat(coglet_fast_loop::MAX_INBOX_BYTES));
                (inbox_line(0, ROUTER, ROUTER, p), 0)
            }),
        ),
    ];
    for (name, make) in cases {
        let mut rig = Rig::new(config());
        let id = request_id(&rig.step(&[], Some(obs())).unwrap());
        let (raw, wake) = make(&id);
        let before = rig.state();
        let out = step_with_inbox(&mut rig, &raw, wake).unwrap();
        assert_eq!(out.actions.len(), 1, "{name}: the tick still acts");
        let s = rig.state();
        assert_eq!(s.stats.inbox_faults, 1, "{name}");
        assert!(s.plan.is_none(), "{name}: damaged reply installs nothing");
        assert_eq!(s.pending, before.pending, "{name}: request stays pending");
        assert_eq!(
            s.inbox_cursor,
            wake + 1,
            "{name}: lines skipped, not retried"
        );
        // The next tick also acts and the awaited request still times out normally.
        assert_eq!(
            rig.step(&[], Some(obs())).unwrap().actions.len(),
            1,
            "{name}"
        );
    }
}

#[test]
fn inbox_is_not_read_unless_a_wake_names_a_new_line() {
    let mut rig = Rig::new(config());
    rig.step(&[], Some(obs())).unwrap();
    assert!(!rig.host.reads.iter().any(|p| p == "/data/inbox.jsonl"));
    let id = rig.state().pending.unwrap().request_id;
    rig.step(&[(ROUTER, "llm.reply", plan_reply(&id))], Some(obs()))
        .unwrap();
    assert!(rig.host.reads.iter().any(|p| p == "/data/inbox.jsonl"));
    // A stale wake for the consumed line does not read it again.
    let stale = ev(
        COGLET,
        "runtime.inbox",
        90,
        1,
        json!({"index": 0, "id": corr(0x1000)}),
        &corr(0x1000),
    );
    let obs_ev = ev(SURFACE, "fixture.state", 91, 1, obs(), &corr(7));
    rig.run(9, 9, 42, vec![stale, obs_ev], vec![], true)
        .unwrap();
    assert!(!rig.host.reads.iter().any(|p| p == "/data/inbox.jsonl"));
}

#[test]
fn action_deadline_overflow_is_refused_not_wrapped() {
    let mut rig = Rig::new(config());
    let event = ev(SURFACE, "fixture.state", 1, u64::MAX, obs(), &corr(1));
    let result = rig.run(0, u64::MAX, 42, vec![event], vec![], true);
    assert_eq!(result, Err(HostError::InvalidArgument));
    assert!(rig.host.files.is_empty() && rig.host.calls.is_empty());
}

#[test]
fn zero_backoff_retries_a_refused_request_on_the_next_tick_and_nonzero_waits() {
    for (backoff, expect_gap) in [(0u64, 1u64), (5, 5)] {
        let mut cfg = config();
        cfg.backoff_ticks = backoff;
        let mut rig = Rig::new(cfg);
        rig.host.refuse_llm = true;
        rig.step(&[], Some(obs())).unwrap();
        assert_eq!(rig.state().stats.plan_refused, 1);
        rig.host.refuse_llm = false;
        let mut gap = 0;
        while rig.step(&[], Some(obs())).unwrap().requests.is_empty() {
            gap += 1;
            assert!(gap < 20);
        }
        assert_eq!(gap + 1, expect_gap, "backoff {backoff}");
    }
}

#[test]
fn valid_reply_with_runtime_allocated_id_is_applied() {
    // inbox_line gives the message id a value distinct from the event correlation_id.
    let mut rig = Rig::new(config());
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    step_with_inbox(&mut rig, &inbox_line(0, ROUTER, ROUTER, plan_reply(&id)), 0).unwrap();
    let s = rig.state();
    assert_eq!((s.stats.inbox_faults, s.stats.plans_applied), (0, 1));
    assert!(s.plan.is_some() && s.pending.is_none());
}

fn ticks_until_next_request(rig: &mut Rig, from_tick: u64) -> u64 {
    for _ in 0..40 {
        if !rig.step(&[], Some(obs())).unwrap().requests.is_empty() {
            return rig.state().tick - from_tick;
        }
    }
    panic!("no request within 40 ticks");
}

#[test]
fn provider_failure_and_timeout_back_off_exactly_backoff_ticks() {
    // Failure reply consumed in a step that also ticks: next request backoff ticks later.
    let mut rig = Rig::new(config());
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    let failed = json!({"request_id": id, "status": "failed",
        "error": {"code": "unavailable", "message": "x", "retryable": true}});
    rig.step(&[(ROUTER, "llm.reply", failed)], Some(obs()))
        .unwrap();
    let at = rig.state().tick;
    assert_eq!(ticks_until_next_request(&mut rig, at), 10);

    // Timeout fires inside the tick at issued + timeout + 1; next request backoff later.
    let mut rig = Rig::new(config());
    rig.step(&[], Some(obs())).unwrap();
    while rig.state().stats.plan_timeouts == 0 {
        rig.step(&[], Some(obs())).unwrap();
    }
    let at = rig.state().tick;
    assert_eq!(at, 1 + 60 + 1);
    assert_eq!(ticks_until_next_request(&mut rig, at), 10);
}

#[test]
fn extreme_counters_and_config_saturate_instead_of_wrapping_or_panicking() {
    // Backoff of u64::MAX must disable retries, not wrap to an immediate retry.
    let mut cfg = config();
    cfg.backoff_ticks = u64::MAX;
    let mut rig = Rig::new(cfg);
    rig.host.refuse_llm = true;
    rig.step(&[], Some(obs())).unwrap();
    rig.host.refuse_llm = false;
    for _ in 0..30 {
        assert!(rig.step(&[], Some(obs())).unwrap().requests.is_empty());
    }
    // Maxed tick/cursor/pending state neither panics nor changes behaviour.
    let mut cfg = config();
    cfg.plan_ttl_ticks = u64::MAX;
    cfg.plan_timeout_ticks = u64::MAX;
    let mut rig = Rig::new(cfg);
    let mut state: State<Waypoints> = State {
        tick: u64::MAX,
        inbox_cursor: u64::MAX,
        next_plan_tick: u64::MAX,
        plan_tick: u64::MAX,
        ..State::default()
    };
    state.stats.ticks = u64::MAX;
    rig.host
        .files
        .insert(STATE.into(), state.to_bytes().unwrap());
    assert_eq!(rig.step(&[], Some(obs())).unwrap().actions.len(), 1);
    let s = rig.state();
    assert_eq!((s.tick, s.stats.ticks), (u64::MAX, u64::MAX));
    // A wake past a maxed cursor saturates the new cursor instead of overflowing.
    let wake = ev(
        COGLET,
        "runtime.inbox",
        9,
        1,
        json!({"index": u64::MAX, "id": corr(5)}),
        &corr(5),
    );
    rig.run(1, 1, 42, vec![wake], vec![], true).unwrap();
    assert_eq!(rig.state().inbox_cursor, u64::MAX);
}

#[test]
fn lines_after_the_woken_index_are_neither_validated_nor_applied() {
    let mut rig = Rig::new(config());
    let id = request_id(&rig.step(&[], Some(obs())).unwrap());
    let outcome =
        json!({"id": corr(1), "correlation_id": corr(2), "status": "succeeded", "result": {}});
    let mut raw = inbox_kind_line(0, "action.outcome", ROUTER, outcome, &corr(2));
    raw.extend(inbox_line(1, ROUTER, ROUTER, plan_reply(&id)));
    raw.extend(b"not json at all\n");
    // Wake for line 0 only: a valid action.outcome is ignored without fault; the reply on
    // line 1 is not applied yet and the garbage on line 2 is not even parsed.
    step_with_inbox(&mut rig, &raw, 0).unwrap();
    let s = rig.state();
    assert_eq!(
        (s.stats.inbox_faults, s.stats.ignored, s.inbox_cursor),
        (0, 1, 1)
    );
    assert!(s.plan.is_none() && s.pending.is_some());
    // The wake for line 1 applies the reply, and line 2 is still not read.
    step_with_inbox(&mut rig, &raw, 1).unwrap();
    let s = rig.state();
    assert_eq!(
        (s.stats.inbox_faults, s.stats.plans_applied, s.inbox_cursor),
        (0, 1, 2)
    );
}

#[test]
fn budget_charge_refuses_without_overflow_or_partial_charge() {
    let mut b = Budget::new(10);
    assert!(b.charge(7).is_ok());
    assert_eq!(b.charge(4), Err(Exhausted));
    assert_eq!(b.used(), 7, "a refused charge costs nothing");
    assert_eq!(b.charge(u64::MAX), Err(Exhausted));
    assert!(b.charge(3).is_ok());
    assert_eq!((b.used(), b.limit()), (10, 10));
    assert_eq!(b.charge(1), Err(Exhausted));
}

#[test]
fn bad_observations_are_counted_and_skipped_without_effects() {
    let mut rig = Rig::new(config());
    let out = rig.step(&[], Some(json!({"x": "nope"}))).unwrap();
    assert!(out.actions.is_empty() && out.requests.is_empty());
    let s = rig.state();
    assert_eq!((s.stats.bad_observations, s.tick), (1, 0));
}
