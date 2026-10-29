use std::path::PathBuf;

use clap::Parser;
use flock::{AppearanceArg, Object, Preset, Screen, parse_object};
use flock_core::ClusterId;
use flock_core::app_settings::Highlight;
use flock_shots::{Highlights, Ui};
use flock_ui::kit::GROUP_SLOT;

#[derive(Parser, Debug)]
#[command(
    name = "flock-shots",
    about = "Render a Flock demo screen to a PNG without a window"
)]
struct Args {
    #[arg(value_enum)]
    screen: Screen,
    #[arg(value_enum, default_value_t = AppearanceArg::Dark)]
    appearance: AppearanceArg,
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
    #[arg(long, value_enum)]
    state: Option<flock_shots::StateFixture>,
    #[arg(long, value_name = "NAME")]
    cluster: Option<String>,
    #[arg(long, value_name = "NAMESPACE/NAME", value_parser = parse_object)]
    object: Option<Object>,
    #[arg(long, value_name = "PRESET|#HEX", value_parser = parse_highlight)]
    primary: Option<Highlight>,
    #[arg(long, value_name = "PRESET|#HEX", value_parser = parse_highlight)]
    secondary: Option<Highlight>,
    #[arg(long, value_name = "KEY", conflicts_with_all = ["state", "primary", "secondary"])]
    group: Option<String>,
}

fn parse_highlight(input: &str) -> Result<Highlight, String> {
    let preset = Highlight::PRESETS
        .into_iter()
        .find(|preset| preset.label().eq_ignore_ascii_case(input));
    match preset {
        Some(preset) => Ok(preset),
        None => input
            .parse()
            .map(Highlight::Custom)
            .map_err(|error| format!("{input}: a preset name or {error}")),
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let preset = Preset {
        cluster: args.cluster.map(ClusterId::new),
        object: args.object,
    };
    let appearance = args.appearance.into();
    if let Some(key) = args.group {
        let mut ui = Ui::with(args.screen, preset, appearance)?;
        ui.click(GROUP_SLOT)?;
        ui.press(&key)?;
        ui.save_png(&args.out)?;
        println!("{}", args.out.display());
        return Ok(());
    }
    let highlights = Highlights {
        primary: args.primary.unwrap_or_default(),
        secondary: args.secondary.unwrap_or_default(),
    };
    let image =
        flock_shots::render_state(args.screen, &preset, appearance, highlights, args.state)?;
    flock_shots::save(&image, &args.out)?;
    println!("{}", args.out.display());
    Ok(())
}
