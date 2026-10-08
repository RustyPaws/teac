//! Compile context: terminal output (stage headers, timed phases, progress bars, warnings),
//! the `.log` file mirror, and the cancel flag. Shared by all stages and safe to use from
//! rayon worker threads.

use console::{style, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Ctx {
    mp: MultiProgress,
    log: Mutex<String>,
    warnings: AtomicUsize,
    cancel: AtomicBool,
    pub verbose: bool,
    start: Instant,
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx::new(false)
    }
}

impl Ctx {
    pub fn new(verbose: bool) -> Ctx {
        let target = if Term::stderr().is_term() { ProgressDrawTarget::stderr() } else { ProgressDrawTarget::hidden() };
        Ctx {
            mp: MultiProgress::with_draw_target(target),
            log: Mutex::new(String::new()),
            warnings: AtomicUsize::new(0),
            cancel: AtomicBool::new(false),
            verbose,
            start: Instant::now(),
        }
    }

    /// A context that prints nothing (tests).
    pub fn quiet() -> Ctx {
        let c = Ctx::new(false);
        c.mp.set_draw_target(ProgressDrawTarget::hidden());
        c
    }

    fn emit(&self, term: String, plain: String) {
        {
            let mut l = self.log.lock().unwrap();
            l.push_str(&plain);
            l.push('\n');
        }
        if self.mp.is_hidden() {
            if !cfg!(test) {
                eprintln!("{term}");
            }
        } else {
            let _ = self.mp.println(term);
        }
    }

    /// Big header for a compile stage (`vbsp`, `vvis`, `vrad`).
    pub fn stage(&self, name: &str, what: impl Display) {
        let what = what.to_string();
        self.emit(
            format!("\n{} {}", style(format!(" {name} ")).bold().reverse().cyan(), style(&what).bold()),
            format!("==== {name}: {what} ===="),
        );
    }

    pub fn info(&self, msg: impl Display) {
        let m = msg.to_string();
        self.emit(format!("  {} {m}", style("·").dim()), m);
    }

    /// Only shown with `--verbose` (always written to the log).
    pub fn detail(&self, msg: impl Display) {
        let m = msg.to_string();
        if self.verbose {
            self.emit(format!("    {}", style(&m).dim()), m);
        } else {
            let mut l = self.log.lock().unwrap();
            l.push_str(&m);
            l.push('\n');
        }
    }

    pub fn warn(&self, msg: impl Display) {
        self.warnings.fetch_add(1, Ordering::Relaxed);
        let m = msg.to_string();
        self.emit(format!("  {} {}", style("⚠").yellow().bold(), style(&m).yellow()), format!("WARNING: {m}"));
    }

    pub fn error(&self, msg: impl Display) {
        let m = msg.to_string();
        self.emit(format!("  {} {}", style("✗").red().bold(), style(&m).red().bold()), format!("ERROR: {m}"));
    }

    pub fn success(&self, msg: impl Display) {
        let m = msg.to_string();
        self.emit(format!("{} {}", style("✓").green().bold(), style(&m).green().bold()), m);
    }

    /// A timed phase with a spinner; finish it with [`Phase::done`] (or let it drop).
    pub fn phase(&self, name: &str) -> Phase<'_> {
        let pb = self.mp.add(ProgressBar::new_spinner());
        pb.set_style(ProgressStyle::with_template("  {spinner:.cyan} {msg} {elapsed:.dim}").unwrap());
        pb.set_message(name.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Phase { ctx: self, name: name.to_string(), pb, start: Instant::now(), note: None }
    }

    /// A timed phase with a progress bar of `total` steps (call `inc` from any thread).
    pub fn progress(&self, name: &str, total: u64) -> Phase<'_> {
        let pb = self.mp.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} {msg:<24} [{bar:30.cyan/blue}] {pos}/{len} {eta:.dim}")
                .unwrap()
                .progress_chars("━╸ "),
        );
        pb.set_message(name.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        Phase { ctx: self, name: name.to_string(), pb, start: Instant::now(), note: None }
    }

    pub fn warning_count(&self) -> usize {
        self.warnings.load(Ordering::Relaxed)
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn log_text(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub struct Phase<'a> {
    ctx: &'a Ctx,
    name: String,
    pb: ProgressBar,
    start: Instant,
    note: Option<String>,
}

impl Phase<'_> {
    pub fn inc(&self, n: u64) {
        self.pb.inc(n);
    }

    pub fn set_len(&self, n: u64) {
        self.pb.set_length(n);
    }

    /// Text shown after the phase name once it finishes ("1234 leafs").
    pub fn note(&mut self, s: impl Display) {
        self.note = Some(s.to_string());
    }

    pub fn done(self) {}
}

impl Drop for Phase<'_> {
    fn drop(&mut self) {
        self.pb.finish_and_clear();
        self.ctx.mp.remove(&self.pb);
        let t = fmt_duration(self.start.elapsed());
        let note = self.note.take().unwrap_or_default();
        self.ctx.emit(
            format!("  {} {:<24} {:>8} {}", style("✓").green(), self.name, style(&t).dim(), style(&note).cyan()),
            format!("{} ({t}) {note}", self.name),
        );
    }
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{:.0}ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.2}s")
    } else {
        format!("{}m{:02}s", s as u64 / 60, s as u64 % 60)
    }
}
