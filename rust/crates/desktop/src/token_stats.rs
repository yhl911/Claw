//! Token usage tracking — append-only log under
//! `~/Library/Application Support/opc-desktop/token_log.jsonl`.
//!
//! Every completed turn writes one JSON record:
//! ```json
//! {"ts": 1778290149, "model": "deepseek-v4-flash", "in": 1024, "out": 512}
//! ```
//!
//! Aggregation is done in-memory at read time. We don't bother with a
//! database; with one record per turn even years of heavy use stays well
//! under a megabyte. If it ever becomes a problem we can rotate.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    ts: u64,
    model: String,
    #[serde(rename = "in")]
    input: u64,
    #[serde(rename = "out")]
    output: u64,
    /// Number of runtime iterations (assistant messages) for this turn.
    /// Optional — older log lines (before quality telemetry) lack it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    iterations: Option<u32>,
    /// Number of tool_result blocks marked `is_error` for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_errors: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStats {
    pub today_input: u64,
    pub today_output: u64,
    pub week_input: u64,
    pub week_output: u64,
    pub month_input: u64,
    pub month_output: u64,
    pub total_input: u64,
    pub total_output: u64,
    pub turn_count_today: u64,
    /// Estimated USD cost for this month, summed using a generic per-1M
    /// rate. The model field can vary per turn; we apply a per-model
    /// pricing table when known and a default rate otherwise.
    pub month_cost_usd: f64,
}

fn token_log_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("opc-desktop").join("token_log.jsonl")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append a single turn's token usage to the log. Best-effort — failures
/// here should never break a successful turn, so the caller logs and
/// otherwise ignores errors.
#[allow(dead_code)]
pub fn record_turn(model: &str, input_tokens: u64, output_tokens: u64) -> std::io::Result<()> {
    record_turn_full(model, input_tokens, output_tokens, None, None)
}

/// Extended logger that also captures quality signals: iteration count
/// (number of assistant messages produced inside the runtime's inner loop)
/// and tool error count (tool_result blocks marked is_error this turn).
/// Both are optional so legacy callers stay unaffected.
pub fn record_turn_full(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    iterations: Option<u32>,
    tool_errors: Option<u32>,
) -> std::io::Result<()> {
    if input_tokens == 0 && output_tokens == 0 {
        return Ok(()); // nothing to record
    }
    let path = token_log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let record = TokenRecord {
        ts: now_secs(),
        model: model.to_string(),
        input: input_tokens,
        output: output_tokens,
        iterations,
        tool_errors,
    };
    let mut line = serde_json::to_string(&record).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// Per-model price table: USD per 1M tokens (input, output). Approximate
/// rates as of writing. Unknown models fall back to `DEFAULT_RATE`.
const DEFAULT_INPUT_USD_PER_M: f64 = 3.0;
const DEFAULT_OUTPUT_USD_PER_M: f64 = 15.0;

fn model_rate(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    // Strip provider prefix if present
    let m = m.strip_prefix("openai/").unwrap_or(&m);
    match m {
        x if x.contains("opus") => (15.0, 75.0),
        x if x.contains("sonnet") => (3.0, 15.0),
        x if x.contains("haiku") => (0.8, 4.0),
        x if x.contains("deepseek-v") => (0.14, 0.28),
        x if x.contains("deepseek-chat") => (0.14, 0.28),
        x if x.contains("deepseek-reasoner") => (0.55, 2.19),
        x if x.contains("gpt-4o") => (2.5, 10.0),
        x if x.contains("gpt-4") => (10.0, 30.0),
        x if x.contains("gpt-3.5") => (0.5, 1.5),
        x if x.contains("qwen") => (0.4, 1.2),
        x if x.contains("grok") => (5.0, 15.0),
        _ => (DEFAULT_INPUT_USD_PER_M, DEFAULT_OUTPUT_USD_PER_M),
    }
}

const SECS_PER_DAY: u64 = 86_400;
const SECS_PER_WEEK: u64 = 7 * SECS_PER_DAY;
const SECS_PER_MONTH: u64 = 30 * SECS_PER_DAY;

/// Sum USD cost for turns within the last `window_secs` seconds. Cheap
/// enough to call before every turn — token log is tiny (one line per
/// turn) and we keep the reader in one pass.
pub fn cost_within_window_usd(window_secs: u64) -> f64 {
    let path = token_log_path();
    let Ok(file) = std::fs::File::open(&path) else {
        return 0.0;
    };
    let cutoff = now_secs().saturating_sub(window_secs);
    let mut total = 0.0;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<TokenRecord>(&line) else {
            continue;
        };
        if rec.ts < cutoff {
            continue;
        }
        let (in_rate, out_rate) = model_rate(&rec.model);
        #[allow(clippy::cast_precision_loss)]
        let in_cost = (rec.input as f64) * in_rate / 1_000_000.0;
        #[allow(clippy::cast_precision_loss)]
        let out_cost = (rec.output as f64) * out_rate / 1_000_000.0;
        total += in_cost + out_cost;
    }
    total
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelBreakdown {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// `day`, `week`, or `month` — the window the totals cover.
    pub window: String,
    pub total_cost_usd: f64,
    pub total_turns: u64,
    /// Per-model rows sorted descending by cost so the worst spenders
    /// surface at the top of any UI list.
    pub by_model: Vec<ModelBreakdown>,
    /// Last 14 daily totals (oldest first) so a sparkline can be drawn
    /// without a second pass over the log.
    pub daily_history: Vec<DailyCost>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyCost {
    /// Day key, formatted as `YYYY-MM-DD` in local time.
    pub date: String,
    pub cost_usd: f64,
    pub turns: u64,
}

/// Aggregate cost over a trailing window, grouped by model + a 14-day
/// daily history. Single pass over the (line-delimited) log.
pub fn cost_breakdown(window_secs: u64) -> CostBreakdown {
    let path = token_log_path();
    let now = now_secs();
    let cutoff = now.saturating_sub(window_secs);
    let mut out = CostBreakdown {
        window: window_label(window_secs),
        ..Default::default()
    };

    // Pre-seed 14 day buckets so the sparkline always has 14 entries
    // even on days with no activity.
    let history_days = 14u64;
    let mut history: std::collections::BTreeMap<String, DailyCost> =
        std::collections::BTreeMap::new();
    for i in 0..history_days {
        let ts = now.saturating_sub(i * SECS_PER_DAY);
        let date = format_day(ts);
        history.entry(date.clone()).or_insert(DailyCost {
            date,
            cost_usd: 0.0,
            turns: 0,
        });
    }

    let Ok(file) = std::fs::File::open(&path) else {
        out.daily_history = history.into_values().collect();
        return out;
    };

    let mut by_model: std::collections::HashMap<String, ModelBreakdown> =
        std::collections::HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<TokenRecord>(&line) else {
            continue;
        };
        let (in_rate, out_rate) = model_rate(&rec.model);
        #[allow(clippy::cast_precision_loss)]
        let in_cost = (rec.input as f64) * in_rate / 1_000_000.0;
        #[allow(clippy::cast_precision_loss)]
        let out_cost = (rec.output as f64) * out_rate / 1_000_000.0;
        let turn_cost = in_cost + out_cost;

        // Daily history covers the last 14 days regardless of the main
        // window — it's the sparkline, not a window-bounded aggregate.
        if rec.ts >= now.saturating_sub(history_days * SECS_PER_DAY) {
            let day = format_day(rec.ts);
            let entry = history.entry(day.clone()).or_insert(DailyCost {
                date: day,
                cost_usd: 0.0,
                turns: 0,
            });
            entry.cost_usd += turn_cost;
            entry.turns += 1;
        }

        if rec.ts < cutoff {
            continue;
        }
        out.total_cost_usd += turn_cost;
        out.total_turns += 1;
        let entry = by_model
            .entry(rec.model.clone())
            .or_insert(ModelBreakdown {
                model: rec.model.clone(),
                ..Default::default()
            });
        entry.input_tokens += rec.input;
        entry.output_tokens += rec.output;
        entry.turns += 1;
        entry.cost_usd += turn_cost;
    }

    let mut models: Vec<ModelBreakdown> = by_model.into_values().collect();
    models.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    out.by_model = models;
    out.daily_history = history.into_values().collect();
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelQuality {
    pub model: String,
    pub turns: u64,
    /// Average iterations per turn — lower is better (less wandering).
    pub avg_iterations: f64,
    /// Fraction of turns that produced ≥1 tool_error block.
    pub tool_error_rate: f64,
    /// Worst-case iteration count seen.
    pub max_iterations: u32,
    /// How many turns in this window actually carry quality fields. Older
    /// turns predate the telemetry — surfaced so the UI can warn when a
    /// model's row is based on a thin sample.
    pub telemetry_turns: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityBreakdown {
    pub window: String,
    pub by_model: Vec<ModelQuality>,
}

/// Aggregate quality signals from the token log over a trailing window.
/// Single pass; cheap enough to call from a Tauri command on demand.
pub fn quality_breakdown(window_secs: u64) -> QualityBreakdown {
    let path = token_log_path();
    let cutoff = now_secs().saturating_sub(window_secs);
    let mut out = QualityBreakdown {
        window: window_label(window_secs),
        ..Default::default()
    };

    let Ok(file) = std::fs::File::open(&path) else {
        return out;
    };

    #[derive(Default)]
    struct Acc {
        turns: u64,
        telemetry_turns: u64,
        iter_sum: u64,
        max_iter: u32,
        error_turns: u64,
    }
    let mut by_model: std::collections::HashMap<String, Acc> = std::collections::HashMap::new();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<TokenRecord>(&line) else {
            continue;
        };
        if rec.ts < cutoff {
            continue;
        }
        let acc = by_model.entry(rec.model.clone()).or_default();
        acc.turns += 1;
        if let Some(it) = rec.iterations {
            acc.telemetry_turns += 1;
            acc.iter_sum += u64::from(it);
            if it > acc.max_iter {
                acc.max_iter = it;
            }
        }
        if let Some(te) = rec.tool_errors {
            if te > 0 {
                acc.error_turns += 1;
            }
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let mut rows: Vec<ModelQuality> = by_model
        .into_iter()
        .map(|(model, a)| ModelQuality {
            model,
            turns: a.turns,
            avg_iterations: if a.telemetry_turns == 0 {
                0.0
            } else {
                a.iter_sum as f64 / a.telemetry_turns as f64
            },
            tool_error_rate: if a.turns == 0 {
                0.0
            } else {
                a.error_turns as f64 / a.turns as f64
            },
            max_iterations: a.max_iter,
            telemetry_turns: a.telemetry_turns,
        })
        .collect();
    rows.sort_by(|a, b| b.turns.cmp(&a.turns));
    out.by_model = rows;
    out
}

fn window_label(secs: u64) -> String {
    match secs {
        SECS_PER_DAY => "day".to_string(),
        SECS_PER_WEEK => "week".to_string(),
        SECS_PER_MONTH => "month".to_string(),
        other => format!("{other}s"),
    }
}

fn format_day(ts: u64) -> String {
    // UTC day key. Local-time bucketing would need a tz crate; we don't
    // care about midnight rollover precision for a sparkline.
    let days_since_epoch = ts / SECS_PER_DAY;
    // Convert days since 1970-01-01 to a YYYY-MM-DD string via the
    // proleptic Gregorian calendar. Simple Howard Hinnant style algorithm
    // — avoids pulling chrono just for a label.
    let z = days_since_epoch as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
}

/// Result of a budget check, used by the worker before each turn. We
/// return `Err(reason)` when the daily or monthly hard cap would be
/// exceeded so the caller can surface a friendly message to the user
/// rather than letting the LLM call proceed and burn unexpected money.
pub fn check_budget(daily_cap_usd: f64, monthly_cap_usd: f64) -> Result<(), String> {
    if daily_cap_usd > 0.0 {
        let spent = cost_within_window_usd(SECS_PER_DAY);
        if spent >= daily_cap_usd {
            return Err(format!(
                "daily budget exceeded: spent ${spent:.2} of ${daily_cap_usd:.2} limit; raise the cap in Settings to continue"
            ));
        }
    }
    if monthly_cap_usd > 0.0 {
        let spent = cost_within_window_usd(SECS_PER_MONTH);
        if spent >= monthly_cap_usd {
            return Err(format!(
                "monthly budget exceeded: spent ${spent:.2} of ${monthly_cap_usd:.2} limit; raise the cap in Settings to continue"
            ));
        }
    }
    Ok(())
}

/// Read the token log and aggregate into stats buckets.
pub fn read_stats() -> TokenStats {
    let path = token_log_path();
    if !path.exists() {
        return TokenStats::default();
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return TokenStats::default();
    };
    let now = now_secs();
    let day_cutoff = now.saturating_sub(SECS_PER_DAY);
    let week_cutoff = now.saturating_sub(SECS_PER_WEEK);
    let month_cutoff = now.saturating_sub(SECS_PER_MONTH);

    let mut stats = TokenStats::default();
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<TokenRecord>(&line) else {
            continue;
        };
        stats.total_input += rec.input;
        stats.total_output += rec.output;
        if rec.ts >= month_cutoff {
            stats.month_input += rec.input;
            stats.month_output += rec.output;
            let (in_rate, out_rate) = model_rate(&rec.model);
            // Token counts in real conversations stay well under 2^53 so the
            // u64→f64 cast is precision-safe. Suppress the pedantic lint.
            #[allow(clippy::cast_precision_loss)]
            let in_cost = (rec.input as f64) * in_rate / 1_000_000.0;
            #[allow(clippy::cast_precision_loss)]
            let out_cost = (rec.output as f64) * out_rate / 1_000_000.0;
            stats.month_cost_usd += in_cost + out_cost;
        }
        if rec.ts >= week_cutoff {
            stats.week_input += rec.input;
            stats.week_output += rec.output;
        }
        if rec.ts >= day_cutoff {
            stats.today_input += rec.input;
            stats.today_output += rec.output;
            stats.turn_count_today += 1;
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn with_temp_log<F: FnOnce()>(f: F) {
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        // Mac data_dir = HOME/Library/Application Support
        // We'll instead override DATA_DIR-equivalent via XDG var on Linux.
        // For test purposes, just verify model_rate is correct without
        // touching the real filesystem.
        f();
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn rates_known_models() {
        assert_eq!(model_rate("claude-opus-4-7"), (15.0, 75.0));
        assert_eq!(model_rate("claude-sonnet-4"), (3.0, 15.0));
        assert_eq!(model_rate("openai/deepseek-v4-flash"), (0.14, 0.28));
        assert_eq!(model_rate("gpt-4o"), (2.5, 10.0));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn rates_default_for_unknown() {
        let (input, output) = model_rate("some-mystery-model");
        assert_eq!(input, DEFAULT_INPUT_USD_PER_M);
        assert_eq!(output, DEFAULT_OUTPUT_USD_PER_M);
    }

    #[test]
    fn records_skipped_when_zero() {
        // record_turn with both zero should early-return Ok without writing
        with_temp_log(|| {
            let r = record_turn("test-model", 0, 0);
            assert!(r.is_ok());
        });
    }
}
