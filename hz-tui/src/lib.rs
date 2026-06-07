mod app;
mod controls;
mod live_diff;
mod model;
mod render;
mod run;
mod syntax;
#[cfg(test)]
mod tests;
mod theme;

pub use run::{
    benchmark_diff_view, run, run_diff, run_diff_with_live_updates,
    run_diff_with_live_updates_and_syntax,
};
pub use theme::{DiffBenchmarkOptions, DiffBenchmarkReport, SyntaxBenchmarkReport};
