#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::non_ascii_literal)]
#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
#![allow(clippy::option_if_let_else)]

use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use clap::{crate_name, Args, Command, FromArgMatches as _, Parser};
use log::{debug, error, info};
use tokio::io::unix::AsyncFd;
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::Instant;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::xproto::{
    self, ColormapWrapper, ConnectionExt as _, CursorWrapper, Window, WindowWrapper,
};
use x11rb::{atom_manager, properties};
// for change_propertyN()
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::render::{self, ConnectionExt as _, PictType};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

mod backbuffer;
mod config;
mod dialog;
mod errors;
mod event;
mod keyboard;
mod secret;

use errors::{Context as _, Result};
use secret::Passphrase;

pub const CLASS: &str = "SshAskpass";
pub const NAME: &str = crate_name!();

include!(concat!(env!("XASKPASS_BUILD_HEADER_DIR"), "/icon.rs"));

// A collection of the atoms we will need.
atom_manager! {
    pub AtomCollection: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_ICON,
        _NET_WM_ICON_NAME,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        UTF8_STRING,
        CLIPBOARD,
        XSEL_DATA,
        INCR,
    }
}

pub type XId = u32;

pub type Connection = XCBConnection;

fn get_deadline(conn: &Connection, window: Window) -> Result<u128> {
    let has_randr = conn
        .extension_information(randr::X11_EXTENSION_NAME)?
        .is_some();
    let mut min_cycle_deadline = None;
    if has_randr {
        let (major, minor) = randr::X11_XML_VERSION;
        let version_cookie = conn.randr_query_version(major, minor)?;
        if log::log_enabled!(log::Level::Debug) {
            let version = version_cookie.reply()?;
            debug!(
                "randr version {}.{}",
                version.major_version, version.minor_version
            );
        }
        // TODO parse error
        // let randr_conf = conn.randr_get_screen_info(screen.root)?.reply().context("randr_get_screen_info_reply")?;
        //debug!("current refresh rate: {}", randr_conf.rate);
        //(1_000_000.0/f64::from(randr_conf.rate).floor()) as u128
        let screen_resources = conn.randr_get_screen_resources(window)?.reply()?;
        for crtc in screen_resources.crtcs {
            let crtc_info = conn
                .randr_get_crtc_info(crtc, screen_resources.config_timestamp)?
                .reply()?;
            if crtc_info.mode == x11rb::NONE {
                continue;
            }
            for mode in &screen_resources.modes {
                if mode.id == crtc_info.mode {
                    if mode.dot_clock != 0 {
                        let vblank_time = (f64::from(mode.htotal) * f64::from(mode.vtotal))
                            / (f64::from(mode.dot_clock) / 1_000_000.0);
                        debug!(
                            "crtc {}: vblank_time: {}μs, dot_clock: {}, htotal: {}, vtotal: {}",
                            crtc, vblank_time, mode.dot_clock, mode.htotal, mode.vtotal
                        );
                        if let Some(min) = min_cycle_deadline {
                            if vblank_time < min {
                                min_cycle_deadline = Some(vblank_time);
                            }
                        } else {
                            min_cycle_deadline = Some(vblank_time);
                        }
                    }
                    break;
                }
            }
        }
    }
    Ok(min_cycle_deadline.map_or(8000, |f| (f / 2.0).floor() as u128))
}

/// Modified from <https://github.com/psychon/x11rb/blob/master/cairo-example/src/main.rs>
/// Choose a visual to use. This function tries to find a depth=32 visual and falls back to the
/// screen's default visual.
fn choose_visual(conn: &Connection, screen_num: usize) -> Result<(u8, xproto::Visualid)> {
    let depth = 32;
    let screen = &conn.setup().roots[screen_num];

    // Try to use XRender to find a visual with alpha support
    let has_render = conn
        .extension_information(render::X11_EXTENSION_NAME)?
        .is_some();
    if has_render {
        let (major, minor) = render::X11_XML_VERSION;
        let version = conn.render_query_version(major, minor)?;
        if log::log_enabled!(log::Level::Debug) {
            let version = version.reply()?;
            debug!(
                "render version {}.{}",
                version.major_version, version.minor_version
            );
        }
        let formats = conn.render_query_pict_formats()?.reply()?;
        // Find the ARGB32 format that must be supported.
        let format = formats
            .formats
            .iter()
            .filter(|info| (info.type_, info.depth) == (PictType::DIRECT, depth))
            .filter(|info| {
                let d = info.direct;
                (d.red_mask, d.green_mask, d.blue_mask, d.alpha_mask) == (0xff, 0xff, 0xff, 0xff)
            })
            .find(|info| {
                let d = info.direct;
                (d.red_shift, d.green_shift, d.blue_shift, d.alpha_shift) == (16, 8, 0, 24)
            });
        if let Some(format) = format {
            debug!("found pict format");
            // Now we need to find the visual that corresponds to this format
            if let Some(visual) = formats.screens[screen_num]
                .depths
                .iter()
                .flat_map(|d| &d.visuals)
                .find(|v| v.format == format.id)
            {
                debug!("found visual");
                return Ok((format.depth, visual.visual));
            }
        }
    }
    Ok((screen.root_depth, screen.root_visual))
}

/// Find a `xcb_visualtype_t` based on its ID number
fn find_xcb_visualtype(conn: &Connection, visual_id: u32) -> Option<xproto::Visualtype> {
    for root in &conn.setup().roots {
        for depth in &root.allowed_depths {
            for visual in &depth.visuals {
                if visual.visual_id == visual_id {
                    debug!("visual class: {:?}", visual.class);
                    return Some(*visual);
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_lines)]
async fn run_xcontext(
    config: config::Config,
    opts: Opts,
    startup_time: Instant,
) -> Result<Option<Passphrase>> {
    let (conn, screen_num) = XCBConnection::connect(None).context("X11 connect")?;
    let xfd = AsyncFd::new(conn).context("asyncfd failed")?;
    let conn = xfd.get_ref();

    debug!("connected X server");
    let atoms = AtomCollection::new(conn)?;

    conn.prefetch_extension_information(x11rb::protocol::present::X11_EXTENSION_NAME)?;
    conn.prefetch_extension_information(x11rb::protocol::xkb::X11_EXTENSION_NAME)?;
    conn.prefetch_extension_information(x11rb::protocol::render::X11_EXTENSION_NAME)?;
    conn.prefetch_extension_information(x11rb::protocol::randr::X11_EXTENSION_NAME)?;

    conn.flush()?;

    let setup = conn.setup();
    let screen = setup.roots.get(screen_num).expect("unknown screen");

    let (depth, visualid) = if config.depth == 32 {
        choose_visual(conn, screen_num)?
    } else {
        (screen.root_depth, screen.root_visual)
    };
    debug!("window depth: {}", depth);

    let compositor_atom = if depth == 32 {
        conn.prefetch_extension_information(x11rb::protocol::xfixes::X11_EXTENSION_NAME)?;
        let compositor_atom = format!("_NET_WM_CM_S{}", screen_num);
        Some(conn.intern_atom(false, compositor_atom.as_bytes())?)
    } else {
        None
    };

    let visual_type = find_xcb_visualtype(conn, visualid).unwrap();

    let surface = backbuffer::XcbSurface::new(conn, screen.root, depth, &visual_type, 1, 1)?;
    let backbuffer = backbuffer::Backbuffer::new(conn, screen.root, surface)?;
    conn.flush()?;
    let mut dialog = dialog::Dialog::new(
        config.dialog,
        screen,
        // TODO should be private
        &backbuffer.cr,
        opts.label.as_deref(),
        opts.debug,
    )?;
    let (window_width, window_height) = dialog.window_size(&backbuffer.cr);
    debug!("window width: {}, height: {}", window_width, window_height);

    let colormap = if visual_type.visual_id == screen.root_visual {
        None
    } else {
        debug!("visualId requires a new colormap");
        let colormap = ColormapWrapper::create_colormap(
            conn,
            xproto::ColormapAlloc::NONE,
            screen.root,
            visual_type.visual_id,
        )?;
        Some(colormap)
    };

    let window_wrapper = WindowWrapper::create_window(
        conn,
        depth,
        screen.root,
        0, // x
        0, // y
        window_width,
        window_height,
        0, // border_width
        xproto::WindowClass::INPUT_OUTPUT,
        visual_type.visual_id,
        &xproto::CreateWindowAux::new()
            .event_mask(
                xproto::EventMask::EXPOSURE
                    | xproto::EventMask::KEY_PRESS
                    | xproto::EventMask::STRUCTURE_NOTIFY
                    | xproto::EventMask::BUTTON_PRESS
                    | xproto::EventMask::BUTTON_RELEASE
                    | xproto::EventMask::POINTER_MOTION
                    | xproto::EventMask::FOCUS_CHANGE,
            )
            .background_pixmap(xproto::PixmapEnum::NONE)
            .border_pixel(screen.black_pixel)
            .colormap(
                colormap
                    .as_ref()
                    .map_or(screen.default_colormap, ColormapWrapper::colormap),
            ),
    )?;
    let window = window_wrapper.window();

    let atoms = atoms.reply()?;

    let hostname = std::env::var_os("HOSTNAME").unwrap_or_else(gethostname::gethostname);
    let mut title = config.title;
    if config.show_hostname {
        title.push('@');
        title.push_str(&hostname.to_string_lossy());
    }
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        xproto::AtomEnum::WM_NAME,
        xproto::AtomEnum::STRING,
        title.as_bytes(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        xproto::AtomEnum::WM_ICON_NAME,
        xproto::AtomEnum::STRING,
        title.as_bytes(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        atoms._NET_WM_NAME,
        atoms.UTF8_STRING,
        title.as_bytes(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        atoms._NET_WM_ICON_NAME,
        atoms.UTF8_STRING,
        title.as_bytes(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        xproto::AtomEnum::WM_CLASS,
        xproto::AtomEnum::STRING,
        [opts.name.as_bytes(), CLASS.as_bytes()]
            .join(&b'\0')
            .as_slice(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        xproto::AtomEnum::WM_CLIENT_MACHINE,
        xproto::AtomEnum::STRING,
        hostname.as_bytes(),
    )?;
    conn.change_property32(
        xproto::PropMode::REPLACE,
        window,
        atoms._NET_WM_PID,
        xproto::AtomEnum::CARDINAL,
        &[std::process::id()],
    )?;
    conn.change_property32(
        xproto::PropMode::REPLACE,
        window,
        atoms._NET_WM_WINDOW_TYPE,
        xproto::AtomEnum::ATOM,
        &[atoms._NET_WM_WINDOW_TYPE_DIALOG],
    )?;
    // be above of other windows
    conn.change_property32(
        xproto::PropMode::REPLACE,
        window,
        atoms._NET_WM_STATE,
        xproto::AtomEnum::ATOM,
        &[atoms._NET_WM_STATE_ABOVE],
    )?;
    // get a client message instead of connection error when the user closes the window
    conn.change_property32(
        xproto::PropMode::REPLACE,
        window,
        atoms.WM_PROTOCOLS,
        xproto::AtomEnum::ATOM,
        &[atoms.WM_DELETE_WINDOW],
    )?;

    // NOTE cannot set urgent with _NET_WM_STATE_ABOVE
    let wm_hints = properties::WmHints {
        input: Some(true),
        initial_state: Some(properties::WmHintsState::Normal),
        ..properties::WmHints::default()
    };
    // TODO icon?
    wm_hints.set(conn, window)?;

    for (width, height, data) in ICONS {
        let mut icon_data = Vec::with_capacity(8 + data.len());
        icon_data.extend_from_slice(&width.to_ne_bytes());
        icon_data.extend_from_slice(&height.to_ne_bytes());
        icon_data.extend_from_slice(data);
        conn.change_property(
            xproto::PropMode::APPEND,
            window,
            atoms._NET_WM_ICON,
            xproto::AtomEnum::CARDINAL,
            32,
            (icon_data.len() / 4).try_into().unwrap(),
            &icon_data,
        )?;
    }

    let mut size_hints = properties::WmSizeHints {
        size: Some((
            properties::WmSizeHintsSpecification::ProgramSpecified,
            window_width.into(),
            window_height.into(),
        )),
        min_size: Some((window_width.into(), window_height.into())),
        ..properties::WmSizeHints::default()
    };
    if !config.resizable {
        debug!("trying to disable resizing");
        size_hints.max_size = Some((window_width.into(), window_height.into()));
    }
    size_hints.set_normal_hints(conn, window)?;

    debug!("map window");
    conn.map_window(window)?;
    debug!("flush");
    conn.flush()?;

    // Load the slow ones after we have mapped the window

    let (transparency, compositor_atom) = if let Some(compositor_atom) = compositor_atom {
        let compositor_atom = compositor_atom.reply()?.atom;
        let selection = conn.get_selection_owner(compositor_atom)?;
        (
            selection.reply()?.owner != x11rb::NONE,
            Some(compositor_atom),
        )
    } else {
        (false, None)
    };

    let resource_db;
    let cursor_handle = if dialog.uses_cursor() {
        debug!("loading x11 resources");
        resource_db = x11rb::resource_manager::new_from_default(conn)?;
        debug!("initializing cursor handle");
        Some(x11rb::cursor::Handle::new(conn, screen_num, &resource_db)?)
    } else {
        None
    };

    debug!("compositor detected: {}", transparency);
    dialog.set_transparency(transparency);

    debug!("keyboard init");
    let keyboard = keyboard::Keyboard::new(conn)?;
    dialog.set_keyboard(&keyboard);

    let input_cursor = if let Some(cursor_handle) = cursor_handle {
        debug!("cursor init");
        let cursor_handle = cursor_handle.reply()?;
        Some(CursorWrapper::for_cursor(
            conn,
            cursor_handle.load_cursor(conn, "xterm").unwrap(),
        ))
    } else {
        None
    };

    let cycle_deadline = get_deadline(conn, window)?;
    debug!("cycle_deadline: {}μs", cycle_deadline);

    debug!("dialog init");
    let mut backbuffer = backbuffer.reply()?;
    backbuffer.init(window, &mut dialog)?;

    let mut xcontext = event::XContext::new(event::Config {
        keyboard,
        xfd: &xfd,
        backbuffer,
        window: window_wrapper,
        atoms,
        width: window_width,
        height: window_height,
        grab_keyboard: config.grab_keyboard,
        startup_time,
        input_cursor,
        compositor_atom,
        debug: opts.debug,
        cycle_deadline,
    })?;
    debug!("init took {}ms", startup_time.elapsed().as_millis());

    xcontext.run_events(dialog).await
}

#[derive(Parser)]
#[command(
    version = env!("XASKPASS_BUILD_FULL_VERSION"),
    about,
    )]
struct Opts {
    #[arg(long, default_value = NAME)]
    /// The instance name
    name: String,

    #[arg(short, long)]
    /// Quiet; silence all logging, no matter the value of verbosity.
    quiet: bool,

    #[arg(short, long, action = clap::ArgAction::Count)]
    /// Increases the level of verbosity (the max level is -vv)
    verbose: u8,

    #[arg(short, long)]
    /// Configuration file path [default: see below]
    config: Option<PathBuf>,

    #[arg(short, long)]
    /// Include additional and sensitive information in logs.
    debug: bool,

    /// Label in the dialog.
    label: Option<String>,

    /// Output default config to stdout.
    #[arg(long)]
    gen_config: bool,
}

fn run() -> i32 {
    let startup_time = Instant::now();

    let cfg_loader = config::Loader::new();
    let mut help = format!(
        "CONFIGURATION FILE:\n    defaults: {}{}.toml",
        cfg_loader.xdg_dirs.get_config_home().display(),
        NAME,
    );
    for d in cfg_loader.xdg_dirs.get_config_dirs() {
        help.push_str(&format!(",\n              {}/{}.toml", d.display(), NAME));
    }
    let app = Command::new(NAME).after_help(help);
    let app = Opts::augment_args(app);
    let opts = Opts::from_arg_matches(&app.get_matches()).expect("from_arg_matches");

    let mut log = stderrlog::new();
    log.quiet(opts.quiet)
        .verbosity(usize::from(opts.verbose) + 2);
    if opts.debug {
        log.timestamp(stderrlog::Timestamp::Millisecond)
            .show_module_names(true);
    }
    log.init().unwrap();
    debug!(
        "{} {} is starting",
        opts.name,
        env!("XASKPASS_BUILD_FULL_VERSION")
    );

    match run_logged(&cfg_loader, opts, startup_time) {
        Ok(ret) => ret,
        Err(err) => {
            error!("{}", err);
            2
        }
    }
}

fn run_logged(cfg_loader: &config::Loader, opts: Opts, startup_time: Instant) -> Result<i32> {
    if opts.gen_config {
        let cfg = config::Config::default();
        config::Loader::print(&cfg)?;
        return Ok(0);
    }

    debug!("load config");
    let config = if let Some(ref path) = opts.config {
        config::Loader::load_path(path)?
    } else {
        cfg_loader.load()?
    };
    debug!("config loaded");

    dialog::setlocale();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .expect("tokio runtime build failed");

    // bind the return value to prevent it from being dropped
    let _runtime_guard = runtime.enter();
    // Initialize signals soon so objects are dropped properly when a signal is received.
    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    let mut sighup = signal(SignalKind::hangup()).unwrap();
    let mut sigterm = signal(SignalKind::terminate()).unwrap();

    let mut mainret = 1;
    runtime.block_on(async {
        tokio::select! {
            _ = sigint.recv() => {
                info!("got sigint");
            }
            _ = sighup.recv() => {
                info!("got sighup");
            }
            _ = sigterm.recv() => {
                info!("got sigterm");
            }
            ret = run_xcontext(config, opts, startup_time) => {
                match ret? {
                    Some(pass) => {
                        pass.write_stdout().unwrap();
                        mainret = 0;
                    }
                    None => {
                        debug!("cancelled");
                    },
                }
            }
        }
        Ok(()) as Result<()>
    })?;
    debug!("exit");
    Ok(mainret)
}

fn main() {
    // std::process::exit exits without dropping objects so call it in minimal stack
    std::process::exit(run());
}

#[test]
fn verify_app() {
    let app = Command::new(NAME);
    let app = Opts::augment_args(app);
    app.debug_assert();
}
