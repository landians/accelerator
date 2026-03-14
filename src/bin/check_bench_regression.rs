use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug)]
struct CliArgs {
    baseline: PathBuf,
    criterion_dir: PathBuf,
    threshold: f64,
}

#[derive(Debug, Deserialize)]
struct BaselineFile {
    benchmarks: BTreeMap<String, BenchEstimate>,
}

#[derive(Debug, Deserialize)]
struct BenchEstimate {
    mean_ns: f64,
}

fn parse_args() -> Result<CliArgs, String> {
    let mut baseline = PathBuf::from("docs/benchmarks/cache_path_bench.json");
    let mut criterion_dir = PathBuf::from("target/criterion");
    let mut threshold = 0.15_f64;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --baseline".to_string())?;
                baseline = PathBuf::from(value);
            }
            "--criterion-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --criterion-dir".to_string())?;
                criterion_dir = PathBuf::from(value);
            }
            "--threshold" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --threshold".to_string())?;
                threshold = value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid --threshold value: {value}"))?;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument: {unknown}")),
        }
    }

    if !(0.0..=1.0).contains(&threshold) {
        return Err(format!(
            "threshold out of range: {threshold} (expected 0.0..=1.0)"
        ));
    }

    Ok(CliArgs {
        baseline,
        criterion_dir,
        threshold,
    })
}

fn print_help() {
    println!(
        "check_bench_regression\n\nUSAGE:\n  cargo run --bin check_bench_regression -- [--baseline <FILE>] [--criterion-dir <DIR>] [--threshold <RATIO>]\n\nDEFAULTS:\n  --baseline docs/benchmarks/cache_path_bench.json\n  --criterion-dir target/criterion\n  --threshold 0.15"
    );
}

fn collect_estimate_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_estimate_files(&path, files)?;
            continue;
        }

        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "estimates.json")
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "new")
        {
            files.push(path);
        }
    }

    Ok(())
}

fn benchmark_id(criterion_dir: &Path, estimate_file: &Path) -> Option<String> {
    let rel = estimate_file.strip_prefix(criterion_dir).ok()?;
    let mut parts = rel
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }

    parts.truncate(parts.len().saturating_sub(2));
    Some(parts.join("/"))
}

fn read_current_mean(path: &Path) -> Result<f64, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;

    value
        .get("mean")
        .and_then(|mean| mean.get("point_estimate"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("missing mean.point_estimate in {}", path.to_string_lossy()))
}

fn load_current(criterion_dir: &Path) -> Result<BTreeMap<String, f64>, String> {
    if !criterion_dir.exists() {
        return Err(format!(
            "criterion dir not found: {}",
            criterion_dir.display()
        ));
    }

    let mut estimate_files = Vec::new();
    collect_estimate_files(criterion_dir, &mut estimate_files)
        .map_err(|err| format!("failed to walk {}: {err}", criterion_dir.display()))?;

    if estimate_files.is_empty() {
        return Err(format!(
            "no benchmark estimates found under {}",
            criterion_dir.display()
        ));
    }

    let mut current = BTreeMap::new();
    for estimate_file in estimate_files {
        let Some(id) = benchmark_id(criterion_dir, &estimate_file) else {
            continue;
        };
        let mean = read_current_mean(&estimate_file)?;
        current.insert(id, mean);
    }

    if current.is_empty() {
        return Err("no valid benchmark entries collected".to_string());
    }

    Ok(current)
}

fn run(args: CliArgs) -> Result<(), String> {
    if !args.baseline.exists() {
        return Err(format!(
            "baseline file not found: {}",
            args.baseline.display()
        ));
    }

    let baseline_text = fs::read_to_string(&args.baseline)
        .map_err(|err| format!("failed to read {}: {err}", args.baseline.display()))?;
    let baseline: BaselineFile = serde_json::from_str(&baseline_text)
        .map_err(|err| format!("failed to parse {}: {err}", args.baseline.display()))?;

    if baseline.benchmarks.is_empty() {
        return Err("baseline benchmarks are empty".to_string());
    }

    let current = load_current(&args.criterion_dir)?;

    let mut checked = 0usize;
    let mut regressions = Vec::new();

    for (benchmark_id, base_entry) in &baseline.benchmarks {
        let Some(current_mean) = current.get(benchmark_id) else {
            println!("[WARN] benchmark missing in current run: {benchmark_id}");
            continue;
        };

        checked += 1;
        let base_mean = base_entry.mean_ns;
        if base_mean <= 0.0 {
            println!("[WARN] invalid baseline mean for {benchmark_id}: {base_mean}");
            continue;
        }

        let ratio = (current_mean - base_mean) / base_mean;
        let ratio_pct = ratio * 100.0;
        println!(
            "[CHECK] {benchmark_id:<30} base={base_mean:>10.2}ns now={current_mean:>10.2}ns delta={ratio_pct:>7.2}%"
        );

        if ratio > args.threshold {
            regressions.push((benchmark_id.clone(), base_mean, *current_mean, ratio_pct));
        }
    }

    if checked == 0 {
        return Err("no overlapping benchmarks between baseline and current run".to_string());
    }

    if !regressions.is_empty() {
        println!("\n[FAIL] performance regression exceeded threshold:");
        for (benchmark_id, base_mean, current_mean, ratio_pct) in regressions {
            println!(
                "  - {benchmark_id}: baseline={base_mean:.2}ns current={current_mean:.2}ns delta={ratio_pct:.2}%"
            );
        }
        return Err("regression detected".to_string());
    }

    println!(
        "\n[PASS] no regression over threshold {:.2}%",
        args.threshold * 100.0
    );

    Ok(())
}

fn main() {
    let args = parse_args().unwrap_or_else(|err| {
        eprintln!("error: {err}");
        print_help();
        std::process::exit(2);
    });

    if let Err(err) = run(args) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
