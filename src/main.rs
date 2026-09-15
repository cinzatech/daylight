//! daylight — terminal day/night world clock (see PLAN.md).
mod app;
mod cli;
mod coast;
mod coast_data;
mod frame;
mod geo;
mod raster;
mod solar;
mod term;

fn main() {
    let cfg = cli::parse();
    let code = app::run(&cfg);
    std::process::exit(code);
}
