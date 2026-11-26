#![deny(warnings)]
//! `coglet-unforged-proof`: runs Acts 0-4 against one unforged repository and
//! prints a report. Exit status 0 only if every check and `git fsck --full` pass.
use coglet_unforged_proof::report::{ActReport, CounterState};
use coglet_unforged_proof::{act0, act1, act2, act3, act4, counter, unf};
use gecaos::os::GecaosOs;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn failed(id: &str, title: &str, err: String) -> ActReport {
    let mut r = ActReport::new(id, title);
    r.check("act ran to completion", false, err);
    r.finish()
}

fn short(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() > n {
        format!("{}…", one_line.chars().take(n).collect::<String>())
    } else {
        one_line
    }
}

fn print_report(acts: &[ActReport], header: &[(String, String)], verbose: bool) {
    let bar = "=".repeat(100);
    println!("{bar}\nCoglet x unforged proof spike\n{bar}");
    for (k, v) in header {
        println!("  {k:<16} {v}");
    }
    println!("{bar}");
    println!(
        "{:<5} {:<64} {:>7} {:>9}  RESULT",
        "ACT", "TITLE", "CHECKS", "TIME"
    );
    for a in acts {
        let passed = a.checks.iter().filter(|c| c.pass).count();
        println!(
            "{:<5} {:<64} {:>3}/{:<3} {:>7}ms  {}",
            a.id,
            short(&a.title, 64),
            passed,
            a.checks.len(),
            a.millis,
            if a.passed() { "PASS" } else { "FAIL" }
        );
    }
    for a in acts {
        println!("\n--- Act {}: {} ---", a.id, a.title);
        for c in &a.checks {
            if verbose || !c.pass {
                println!(
                    "  [{}] {} :: {}",
                    if c.pass { "PASS" } else { "FAIL" },
                    c.name,
                    short(&c.detail, 140)
                );
            } else {
                println!("  [PASS] {}", c.name);
            }
        }
        if !a.hashes.is_empty() {
            println!("  SHA-256 ids:");
            for (label, h) in &a.hashes {
                println!("    {h}  {label}");
            }
        }
        for n in &a.notes {
            println!("  note: {}", short(n, 160));
        }
    }
}

fn to_json(acts: &[ActReport], header: &[(String, String)], ok: bool) -> serde_json::Value {
    json!({
        "pass": ok,
        "header": header.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
        "acts": acts.iter().map(|a| json!({
            "id": a.id, "title": a.title, "pass": a.passed(), "millis": a.millis as u64,
            "checks": a.checks.iter().map(|c| json!({"name": c.name, "pass": c.pass, "detail": c.detail})).collect::<Vec<_>>(),
            "hashes": a.hashes.iter().map(|(l, h)| json!({"label": l, "sha256": h})).collect::<Vec<_>>(),
            "notes": a.notes,
        })).collect::<Vec<_>>(),
    })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn to_html(acts: &[ActReport], header: &[(String, String)], ok: bool) -> String {
    let mut h = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Coglet x unforged proof</title>\
         <style>body{font:14px system-ui,sans-serif;margin:2em;max-width:1200px}\
         table{border-collapse:collapse;width:100%;margin:.5em 0 1.5em}td,th{border:1px solid #ddd;padding:4px 8px;text-align:left;vertical-align:top}\
         .p{color:#137333;font-weight:600}.f{color:#c5221f;font-weight:600}code{font-size:12px;word-break:break-all}</style></head><body>",
    );
    h.push_str(&format!(
        "<h1>Coglet × unforged proof: <span class=\"{}\">{}</span></h1><table>",
        if ok { "p" } else { "f" },
        if ok { "PASS" } else { "FAIL" }
    ));
    for (k, v) in header {
        h.push_str(&format!(
            "<tr><th>{}</th><td><code>{}</code></td></tr>",
            esc(k),
            esc(v)
        ));
    }
    h.push_str("</table><table><tr><th>Act</th><th>Title</th><th>Checks</th><th>Time</th><th>Result</th></tr>");
    for a in acts {
        let passed = a.checks.iter().filter(|c| c.pass).count();
        h.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{passed}/{}</td><td>{} ms</td><td class=\"{}\">{}</td></tr>",
            esc(&a.id),
            esc(&a.title),
            a.checks.len(),
            a.millis,
            if a.passed() { "p" } else { "f" },
            if a.passed() { "PASS" } else { "FAIL" }
        ));
    }
    h.push_str("</table>");
    for a in acts {
        h.push_str(&format!(
            "<h2>Act {}: {}</h2><table>",
            esc(&a.id),
            esc(&a.title)
        ));
        for c in &a.checks {
            h.push_str(&format!(
                "<tr><td class=\"{}\">{}</td><td>{}</td><td><code>{}</code></td></tr>",
                if c.pass { "p" } else { "f" },
                if c.pass { "PASS" } else { "FAIL" },
                esc(&c.name),
                esc(&c.detail)
            ));
        }
        h.push_str("</table>");
        if !a.hashes.is_empty() {
            h.push_str("<table><tr><th>SHA-256</th><th>What</th></tr>");
            for (l, x) in &a.hashes {
                h.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{}</td></tr>",
                    esc(x),
                    esc(l)
                ));
            }
            h.push_str("</table>");
        }
        for n in &a.notes {
            h.push_str(&format!("<p><code>{}</code></p>", esc(n)));
        }
    }
    h.push_str("</body></html>\n");
    h
}

fn run(args: &[String]) -> Result<bool, String> {
    let need = |n: &str| arg(args, n).ok_or_else(|| format!("missing {n}"));
    let work = PathBuf::from(need("--work")?);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let wasm = PathBuf::from(need("--wasm")?);
    let helper = std::fs::read(need("--helper")?).map_err(|e| format!("helper: {e}"))?;
    let setup = counter::build_package(
        &wasm,
        Path::new(&need("--manifest")?),
        &need("--coglet-head")?,
        &work.join("package"),
    )?;
    let genesis = counter::genesis_files(&setup)?;
    let os = GecaosOs::init(work.join("unforged-os"))?;
    let raw = std::fs::read(&wasm).map_err(|e| e.to_string())?;
    let mut header: Vec<(String, String)> = vec![
        (
            "coglet".into(),
            arg(args, "--coglet-rev").unwrap_or_default(),
        ),
        (
            "unforged".into(),
            arg(args, "--unforged-rev").unwrap_or_default(),
        ),
        ("git".into(), arg(args, "--git-version").unwrap_or_default()),
        (
            "guest".into(),
            format!(
                "coglet_counter.wasm sha256:{}",
                gecaos::coglet::sha256_hex(&raw)
            ),
        ),
        (
            "step.wasm".into(),
            format!(
                "R3-canonicalized by coglet_cli::build sha256:{}",
                gecaos::coglet::sha256_hex(&genesis[counter::STEP_WASM_PATH])
            ),
        ),
        ("engine".into(), unf::engine_pin(&helper, "")),
        ("repository".into(), os.git.git_dir.display().to_string()),
    ];

    let mut acts = Vec::new();
    acts.push(
        act0::run(&os, &genesis, &work.join("act0"))
            .unwrap_or_else(|e| failed("0", "ADR-0035 parity", e)),
    );
    let a1 = act1::run(&os, "counter", &genesis, &helper);
    let (chain, resolver) = match a1 {
        Ok(out) => {
            acts.push(out.report);
            (Some(out.chain), out.resolver)
        }
        Err(e) => {
            acts.push(failed("1", "Stage 1 first test", e));
            (None, String::new())
        }
    };
    if let Some(chain) = &chain {
        acts.push(
            act2::run(&os, &genesis, &resolver, &work.join("act2"))
                .unwrap_or_else(|e| failed("2", "Stage 2 chain replay + signatures", e)),
        );
        let tip = chain.commits.last().cloned().unwrap_or_default();
        let wasm_bytes = genesis[counter::STEP_WASM_PATH].clone();
        let dir3 = work.join("act3");
        let dir4 = work.join("act4");
        for d in [&dir3, &dir4] {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let cs = |dir| CounterState {
            os: &os,
            coglet_id: "counter",
            state_0: &chain.state_0,
            step_commits: &chain.commits,
            tip: &tip,
            step_wasm: &wasm_bytes,
            work_dir: dir,
        };
        acts.push(act3::run(&cs(&dir3)).unwrap_or_else(|e| failed("3", "Stage 3 router", e)));
        acts.push(act4::run(&cs(&dir4)).unwrap_or_else(|e| failed("4", "Stage 4 adapters", e)));
    }
    let mut fin = ActReport::new("F", "Repository integrity");
    let (fsck_ok, fsck_out) = unf::fsck(&os);
    fin.check(
        "git fsck --full on the shared unforged repository",
        fsck_ok,
        fsck_out,
    );
    acts.push(fin.finish());

    let ok = acts.iter().all(ActReport::passed);
    let total: usize = acts.iter().map(|a| a.checks.len()).sum();
    header.push((
        "total".into(),
        format!("{total} checks, {} ms", started.elapsed().as_millis()),
    ));
    print_report(&acts, &header, args.iter().any(|a| a == "--verbose"));
    if let Some(p) = arg(args, "--json") {
        let v = to_json(&acts, &header, ok);
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!("\njson report: {p}");
    }
    if let Some(p) = arg(args, "--html") {
        std::fs::write(&p, to_html(&acts, &header, ok)).map_err(|e| e.to_string())?;
        println!("html report: {p}");
    }
    let passed: usize = acts
        .iter()
        .map(|a| a.checks.iter().filter(|c| c.pass).count())
        .sum();
    println!(
        "\n{}: {passed}/{total} checks passed",
        if ok { "PROOF PASSED" } else { "PROOF FAILED" }
    );
    Ok(ok)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("coglet-unforged-proof: {e}");
            std::process::exit(2);
        }
    }
}
