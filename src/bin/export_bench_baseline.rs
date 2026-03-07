use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug)]
struct CliArgs {
    criterion_dir: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Serialize)]
struct BaselineFile {
    version: u32,
    benchmarks: BTreeMap<String, BenchEstimate>,
}

#[derive(Debug, Serialize)]
struct BenchEstimate {
    mean_ns: f64,
    median_ns: f64,
}

fn parse_args() -> Result<CliArgs, String> {
    let mut criterion_dir = PathBuf::from("target/criterion");
    let mut output = PathBuf::from("docs/benchmarks/cache_path_bench.json");

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--criterion-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --criterion-dir".to_string())?;
                criterion_dir = PathBuf::from(value);
            }
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --output".to_string())?;
                output = PathBuf::from(value);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}"));
            }
        }
    }

    Ok(CliArgs {
        criterion_dir,
        output,
    })
}

fn print_help() {
    println!(
        "export_bench_baseline\n\nUSAGE:\n  cargo run --bin export_bench_baseline -- [--criterion-dir <DIR>] [--output <FILE>]\n\nDEFAULTS:\n  --criterion-dir target/criterion\n  --output docs/benchmarks/cache_path_bench.json"
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

fn read_estimate(path: &Path) -> Result<BenchEstimate, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;

    let mean = value
        .get("mean")
        .and_then(|mean| mean.get("point_estimate"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("missing mean.point_estimate in {}", path.to_string_lossy()))?;

    let median = value
        .get("median")
        .and_then(|median| median.get("point_estimate"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            format!(
                "missing median.point_estimate in {}",
                path.to_string_lossy()
            )
        })?;

    Ok(BenchEstimate {
        mean_ns: mean,
        median_ns: median,
    })
}

fn run(args: CliArgs) -> Result<(), String> {
    if !args.criterion_dir.exists() {
        return Err(format!(
            "criterion dir not found: {}",
            args.criterion_dir.display()
        ));
    }

    let mut estimate_files = Vec::new();
    collect_estimate_files(&args.criterion_dir, &mut estimate_files)
        .map_err(|err| format!("failed to walk {}: {err}", args.criterion_dir.display()))?;

    if estimate_files.is_empty() {
        return Err(format!(
            "no benchmark estimates found under {}",
            args.criterion_dir.display()
        ));
    }

    let mut benchmarks = BTreeMap::new();
    for estimate_file in estimate_files {
        let Some(id) = benchmark_id(&args.criterion_dir, &estimate_file) else {
            continue;
        };
        let estimate = read_estimate(&estimate_file)?;
        benchmarks.insert(id, estimate);
    }

    if benchmarks.is_empty() {
        return Err("no valid benchmark entries collected".to_string());
    }

    let payload = BaselineFile {
        version: 1,
        benchmarks,
    };

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    let output_text = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize baseline json: {err}"))?;
    fs::write(&args.output, format!("{output_text}\n"))
        .map_err(|err| format!("failed to write {}: {err}", args.output.display()))?;

    println!(
        "wrote baseline: {} ({} benchmarks)",
        args.output.display(),
        payload.benchmarks.len()
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
