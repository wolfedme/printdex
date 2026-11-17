//! Runs the built `printdex` binary end to end.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn printdex(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_printdex"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

fn library(dir: &Path) -> String {
    let root = dir.join("lib");
    touch(&root, "Red Dragon/a.stl");
    root.to_str().unwrap().to_owned()
}

#[test]
fn scan_then_search_prints_hit() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    let o = printdex(&data, &["scan", &lib]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("models: +1"), "{}", stdout(&o));
    let o = printdex(&data, &["search", "dragon"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("Red Dragon\tRed Dragon\t1 parts"),
        "{}",
        stdout(&o)
    );
    let o = printdex(&data, &["search", "nothing-like-this"]);
    assert!(o.status.success());
    assert!(stdout(&o).is_empty());
}

#[test]
fn second_scan_fingerprints_nothing() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    assert!(printdex(&data, &["scan", &lib]).status.success());
    let o = printdex(&data, &["scan", &lib]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("fingerprinted 0"), "{}", stdout(&o));
}

#[test]
fn scan_missing_path_fails() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("nope");
    let o = printdex(&d.path().join("data"), &["scan", missing.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("error:"), "{}", stderr(&o));
}

#[test]
fn stats_prints_counts() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    assert!(printdex(&data, &["scan", &lib]).status.success());
    let o = printdex(&data, &["stats"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("models: 1"), "{out}");
    assert!(out.contains("parts: 1"), "{out}");
}

fn bench_gen(data: &Path, dir: &Path, parts: &str, seed: &str) {
    let o = printdex(
        data,
        &[
            "bench",
            "gen",
            dir.to_str().unwrap(),
            "--parts",
            parts,
            "--seed",
            seed,
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
}

fn listing(root: &Path) -> Vec<(String, u64)> {
    fn rec(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                rec(root, &p, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, e.metadata().unwrap().len()));
            }
        }
    }
    let mut out = Vec::new();
    rec(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn generator_is_deterministic() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    bench_gen(&data, &d.path().join("a"), "500", "7");
    bench_gen(&data, &d.path().join("b"), "500", "7");
    let a = listing(&d.path().join("a"));
    assert!(!a.is_empty());
    assert_eq!(a, listing(&d.path().join("b")));
    bench_gen(&data, &d.path().join("c"), "500", "8");
    assert_ne!(a, listing(&d.path().join("c")));
}

#[test]
fn generator_hits_requested_part_count() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = d.path().join("lib");
    bench_gen(&data, &lib, "500", "1");
    let o = printdex(&data, &["scan", lib.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = printdex(&data, &["stats"]);
    assert!(stdout(&o).contains("parts: 500\n"), "{}", stdout(&o));
}

#[test]
fn generated_stl_is_valid_binary_stl() {
    let d = tempfile::tempdir().unwrap();
    let lib = d.path().join("lib");
    bench_gen(&d.path().join("data"), &lib, "200", "3");
    let stls: Vec<_> = listing(&lib)
        .into_iter()
        .filter(|(p, _)| p.to_ascii_lowercase().ends_with(".stl"))
        .collect();
    assert!(!stls.is_empty());
    for (rel, _) in stls.iter().take(20) {
        let bytes = fs::read(lib.join(rel)).unwrap();
        let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert!(n >= 1);
        assert_eq!(bytes.len(), 84 + 50 * n, "{rel}");
    }
}

#[test]
fn bench_run_reports_zero_refingerprint() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = d.path().join("lib");
    bench_gen(&data, &lib, "500", "1");
    let o = printdex(
        &data,
        &["bench", "run", lib.to_str().unwrap(), "--queries", "50"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("rescan_fingerprinted: 0"), "{out}");
    assert!(out.contains("search_p95_us: "), "{out}");
}
