//! The output contract (`.design/2026-10-19-output-contract.md`): the
//! fields `--json` prints, per schema number. A field added, removed or
//! renamed without the schema number moving fails here; moving the number
//! means recording the new fields here and in the contract.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use andon::adapters;
use andon::fold::board::board;
use andon::fold::scorecard::scorecard;
use andon::ledger::Ledger;
use andon::time::{Window, parse_instant};
use serde_json::Value;

/// A ledger that fills every field: each kind, each optional value.
const LEDGER: &str = r#"{"t":"1970-01-01T00:00:00Z","kind":"coverage","outcome":"pass","source":"test","attrs":{"covers":["land","commit","run","gate_run","verdict","message","create","claim","close","session_active","unlanded","other:comment"]}}
{"t":"2026-10-13T07:00:00Z","kind":"create","actor":"x-1a","subject":"x-abc","ids":{"goal":"x-abc"},"source":"t","attrs":{"budget":"4 agent-hours","item_type":"task","title":"t"}}
{"t":"2026-10-13T07:30:00Z","kind":"claim","actor":"x-1a","subject":"x-abc","ids":{"goal":"x-abc"},"source":"t"}
{"t":"2026-10-13T07:35:00Z","kind":"create","actor":"Shared","subject":"x-def","ids":{"goal":"x-def"},"source":"t","attrs":{"item_type":"task"}}
{"t":"2026-10-13T07:40:00Z","kind":"claim","actor":"Shared","subject":"x-def","ids":{"goal":"x-def"},"source":"t"}
{"t":"2026-10-13T08:00:00Z","t_end":"2026-10-13T09:00:00Z","kind":"session_active","actor":"x-1a","ids":{"session":"s1"},"source":"t"}
{"t":"2026-10-13T08:10:00Z","kind":"commit","actor":"x-1a","subject":"x-abc","ids":{"commit":"c1","goal":"x-abc"},"source":"t"}
{"t":"2026-10-13T08:20:00Z","t_end":"2026-10-13T08:25:00Z","kind":"run","actor":"x-1a","outcome":"fail","ids":{"commit":"c1"},"source":"t","attrs":{"recipe":"check","repo":"x"}}
{"t":"2026-10-13T08:30:00Z","t_end":"2026-10-13T08:31:00Z","kind":"gate_run","actor":"x-1a","outcome":"pass","source":"t","attrs":{"recipe":"land"}}
{"t":"2026-10-13T08:40:00Z","kind":"verdict","actor":"x-1a","outcome":"go","subject":"x-abc","ids":{"verdict":"v1","commit":"c1"},"source":"t"}
{"t":"2026-10-13T09:00:00Z","kind":"land","actor":"x-1a","outcome":"pass","subject":"x-abc","ids":{"commit":"c1","session":"s1","goal":"x-abc","cites":["v1"]},"source":"t"}
{"t":"2026-10-13T09:10:00Z","kind":"message","actor":"x-1a","ids":{"message":"m1"},"source":"t","attrs":{"message_kind":"ask","to":["x-1b"]}}
{"t":"2026-10-13T09:20:00Z","kind":"message","actor":"x-1b","ids":{"message":"m2"},"source":"t","attrs":{"message_kind":"fyi","to":["x-1a"],"re":"m1"}}
{"t":"2026-10-13T09:30:00Z","kind":"other:comment","actor":"x-1b","subject":"x-abc","ids":{"goal":"x-abc"},"source":"t"}
{"t":"2026-10-13T10:00:00Z","kind":"close","subject":"x-old","ids":{"goal":"x-old"},"source":"t"}
{"t":"2026-10-13T12:00:00Z","t_end":"2026-10-13T12:00:00Z","kind":"coverage","outcome":"pass","source":"sessions","attrs":{"covers":["state:presence"]}}
{"t":"2026-10-13T11:00:00Z","t_end":"2026-10-13T12:00:00Z","kind":"presence","actor":"x-1a","ids":{"session":"s1"},"source":"t","attrs":{"status":"idle","cwd":"/x-1a"}}
{"t":"2026-10-13T10:30:00Z","t_end":"2026-10-13T12:00:00Z","kind":"unlanded","actor":"x-1a","source":"t","attrs":{"path":"src/a.rs","workspace":"x-1a"}}
{"t":"2026-10-13T10:40:00Z","t_end":"2026-10-13T12:00:00Z","kind":"unlanded","actor":"x-1b","source":"t","attrs":{"path":"src/a.rs","workspace":"x-1b","snapshot":"2026-10-13T11:59:00Z"}}
"#;

/// Objects keyed by data (a seat, a repository), not by field.
const MAPS: &[&str] = &["seats", "runs_by_repo"];

/// Every field path in a JSON output: arrays and data-keyed maps are `*`.
fn fields(v: &Value, path: &str, out: &mut BTreeSet<String>) {
    let join = |k: &str| {
        if path.is_empty() {
            k.to_owned()
        } else {
            format!("{path}.{k}")
        }
    };
    match v {
        Value::Object(map) if map.len() == 1 && map.contains_key("unknown") => {
            out.insert(path.to_owned());
        }
        Value::Object(map) => {
            let keyed = MAPS.iter().any(|m| path.ends_with(m)) && !path.contains("seats.*.");
            for (k, x) in map {
                let p = if keyed { join("*") } else { join(k) };
                out.insert(p.clone());
                fields(x, &p, out);
            }
        }
        // a list of values is one field; a list of objects has fields
        Value::Array(items) => {
            for x in items.iter().filter(|x| x.is_object()) {
                let p = join("*");
                out.insert(p.clone());
                fields(x, &p, out);
            }
        }
        _ => {}
    }
}

fn paths(v: &Value) -> Vec<String> {
    let mut out = BTreeSet::new();
    fields(v, "", &mut out);
    out.into_iter().collect()
}

fn outputs() -> (Value, Value) {
    let config = common::config("[goal]\n[seats.x-1a]\n[seats.x-1b]\n", Path::new("/"));
    let ledger = adapters::resolve(
        &config,
        Ledger::read(LEDGER.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"))
            .events()
            .to_vec(),
    );
    let at = |s: &str| parse_instant(s).unwrap_or_else(|e| panic!("{e}"));
    let b = board(&ledger, &config, at("2026-10-13T12:00:00Z"));
    // a window ending after the capture fills `asked_to`
    let window = Window::parse("2026-10-13..2026-10-14", at("2026-10-13T12:00:00Z"))
        .unwrap_or_else(|e| panic!("{e}"));
    let c = scorecard(&ledger, &config, window);
    (
        serde_json::to_value(b).unwrap_or_default(),
        serde_json::to_value(c).unwrap_or_default(),
    )
}

/// The board's fields under schema 2.
const BOARD_V2: &[&str] = &[
    "asof",
    "schema",
    "seats",
    "seats.*",
    "seats.*.findings",
    "seats.*.flags",
    "seats.*.goals",
    "seats.*.goals.*",
    "seats.*.goals.*.amount",
    "seats.*.goals.*.budget",
    "seats.*.goals.*.claimed_at",
    "seats.*.goals.*.id",
    "seats.*.goals.*.title",
    "seats.*.goals.*.unit",
    "seats.*.goals.*.used",
    "seats.*.landings",
    "seats.*.last_land",
    "seats.*.last_verdict",
    "seats.*.last_verdict.outcome",
    "seats.*.last_verdict.subject",
    "seats.*.last_verdict.t",
    "seats.*.runs",
    "seats.*.runs_pass",
    "seats.*.runs_since_land",
    "seats.*.seat",
    "unseated",
    "unseated.*",
    "unseated.*.goal",
    "unseated.*.owner",
    "window_hours",
];

/// The board's fields under schema 3: schema 2's, each row's `state` where
/// live sessions were read (andon-3im), and its `overlaps` (andon-tgv).
const BOARD_V3: &[&str] = &[
    "asof",
    "schema",
    "seats",
    "seats.*",
    "seats.*.findings",
    "seats.*.flags",
    "seats.*.goals",
    "seats.*.goals.*",
    "seats.*.goals.*.amount",
    "seats.*.goals.*.budget",
    "seats.*.goals.*.claimed_at",
    "seats.*.goals.*.id",
    "seats.*.goals.*.title",
    "seats.*.goals.*.unit",
    "seats.*.goals.*.used",
    "seats.*.landings",
    "seats.*.last_land",
    "seats.*.last_verdict",
    "seats.*.last_verdict.outcome",
    "seats.*.last_verdict.subject",
    "seats.*.last_verdict.t",
    "seats.*.overlaps",
    "seats.*.overlaps.*",
    "seats.*.overlaps.*.files",
    "seats.*.overlaps.*.seat",
    "seats.*.overlaps.*.snapshot",
    "seats.*.runs",
    "seats.*.runs_pass",
    "seats.*.runs_since_land",
    "seats.*.seat",
    "seats.*.state",
    "seats.*.state.sessions",
    "seats.*.state.since",
    "seats.*.state.status",
    "unseated",
    "unseated.*",
    "unseated.*.goal",
    "unseated.*.owner",
    "window_hours",
];

/// The scorecard's fields under schema 2.
const SCORECARD_V2: &[&str] = &[
    "asked_to",
    "days",
    "flow",
    "flow.gate_median_min",
    "flow.gate_n",
    "flow.lead_median_min",
    "flow.lead_n",
    "flow.review_median_min",
    "flow.review_n",
    "from",
    "hub",
    "hub.decisions_without_goal",
    "hub.inbound",
    "hub.inbound_per_day",
    "hub.reply_median_min",
    "hub.reply_n",
    "hub.seat",
    "joins",
    "joins.cites",
    "joins.cites_resolved",
    "joins.landing_session",
    "joins.landing_session_seen",
    "joins.landings",
    "joins.landings_citing_verdict",
    "joins.runs",
    "joins.runs_attributed",
    "runs_by_repo",
    "runs_by_repo.*",
    "runs_by_repo.*.fail",
    "runs_by_repo.*.fail_without_reason",
    "runs_by_repo.*.other",
    "runs_by_repo.*.pass",
    "runs_by_repo.*.total",
    "schema",
    "seats",
    "seats.*",
    "seats.*.active_hours",
    "seats.*.decisions_without_goal",
    "seats.*.gate_runs",
    "seats.*.items_created",
    "seats.*.landings",
    "seats.*.messages_naming_goal",
    "seats.*.messages_received",
    "seats.*.messages_sent",
    "seats.*.runs",
    "seats.*.runs.fail",
    "seats.*.runs.fail_without_reason",
    "seats.*.runs.other",
    "seats.*.runs.pass",
    "seats.*.runs.total",
    "seats.*.runs_per_landing",
    "seats.*.verdicts_go",
    "seats.*.verdicts_return",
    "seats.*.wip_at_end",
    "to",
    "total",
    "total.active_hours",
    "total.decisions_without_goal",
    "total.gate_runs",
    "total.items_created",
    "total.landings",
    "total.messages_naming_goal",
    "total.messages_received",
    "total.messages_sent",
    "total.runs",
    "total.runs.fail",
    "total.runs.fail_without_reason",
    "total.runs.other",
    "total.runs.pass",
    "total.runs.total",
    "total.runs_per_landing",
    "total.verdicts_go",
    "total.verdicts_return",
    "total.wip_at_end",
    "tracker",
    "tracker.branching",
    "tracker.closed",
    "tracker.created",
    "tracker.open_at_end",
    "tracker.unowned_at_end",
    "unattributed",
    "unattributed.active_hours",
    "unattributed.decisions_without_goal",
    "unattributed.gate_runs",
    "unattributed.items_created",
    "unattributed.landings",
    "unattributed.messages_naming_goal",
    "unattributed.messages_received",
    "unattributed.messages_sent",
    "unattributed.runs",
    "unattributed.runs.fail",
    "unattributed.runs.fail_without_reason",
    "unattributed.runs.other",
    "unattributed.runs.pass",
    "unattributed.runs.total",
    "unattributed.runs_per_landing",
    "unattributed.verdicts_go",
    "unattributed.verdicts_return",
    "unattributed.wip_at_end",
];

/// The board's text columns under schema 3, which the hook's line labels
/// its cells with (schema 2's, and `state`).
const COLUMNS_V3: &[&str] = &[
    "seat",
    "goal",
    "budget",
    "lands",
    "last",
    "runs",
    "pass",
    "since-land",
    "verdict",
    "findings",
    "state",
    "flags",
];

/// The recorded fields for an output's schema number.
fn recorded(what: &str, schema: &Value) -> &'static [&'static str] {
    match (what, schema.as_u64()) {
        ("board", Some(2)) => BOARD_V2,
        ("board", Some(3)) => BOARD_V3,
        ("scorecard", Some(2)) => SCORECARD_V2,
        (_, n) => panic!(
            "{what} schema {n:?} has no recorded fields; record them here and in .design/2026-10-19-output-contract.md"
        ),
    }
}

#[test]
fn json_fields_change_only_with_the_schema_number() {
    let (b, c) = outputs();
    for (what, v) in [("board", &b), ("scorecard", &c)] {
        let expected: Vec<String> = recorded(what, &v["schema"])
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(
            paths(v),
            expected,
            "{what} --json fields changed under schema {}: move `schema` in src/fold/{what}.rs, and record the fields here and in the output contract",
            v["schema"]
        );
    }
}

#[test]
fn the_hooks_labels_are_the_boards_columns() {
    let names: Vec<&str> = andon::render::BOARD.iter().map(|c| c.name).collect();
    assert_eq!(
        names, COLUMNS_V3,
        "the board's columns (and the hook line's labels) changed: move the board's schema"
    );
}
