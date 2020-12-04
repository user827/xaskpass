use std::error::Error as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use clap::{Clap, FromArgMatches as _, IntoApp as _};
use log::{debug, error, info};
use tokio::io::unix::AsyncFd;
use tokio::signal::unix::{signal, SignalKind};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::{atom_manager, properties};
// for change_propertyN()
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

mod backbuffer;
mod config;
mod dialog;
mod errors;
mod event;
mod keyboard;
mod secret;

use errors::{Error, Result, X11ErrorString as _};
use secret::Passphrase;

pub const CLASS: &str = "SshAskpass";

// A collection of the atoms we will need.
atom_manager! {
    pub AtomCollection: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        UTF8_STRING,
    }
}

pub type XId = u32;

#[derive(Debug)]
pub struct Connection {
    pub xfd: AsyncFd<XCBConnection>,
    pub xerr: errors::Builder,
}

impl Connection {
    pub fn new() -> Result<(Self, usize)> {
        let (conn, screen_num) = XCBConnection::connect(None)?;
        let xerr = errors::Builder::new(&conn);
        debug!("preferred screen {}", screen_num);
        let me = Self {
            xerr,
            // There are no reasonable failures, so lets panic
            xfd: AsyncFd::new(conn).expect("asyncfd failed"),
        };
        Ok((me, screen_num))
    }
}

impl std::ops::Deref for Connection {
    type Target = XCBConnection;
    fn deref(&self) -> &Self::Target {
        self.xfd.get_ref()
    }
}

async fn run_xcontext(
    cfg_loader: config::Loader,
    opts: Opts,
    now: Instant,
) -> Result<Option<Passphrase>> {
    let (conn, screen_num) = Connection::new()?;
    let atoms = AtomCollection::new(&*conn)?
        .reply()
        .map_err(|e| conn.xerr_from("atoms", e))?;

    let keyboard = keyboard::Keyboard::new(&conn)?;

    let setup = conn.setup();
    let screen = setup.roots.get(screen_num).expect("unknown screen");

    let instance = opts.name.unwrap_or_else(|| {
        std::env::args()
            .next()
            .as_ref()
            .and_then(|f| Path::new(f).file_name()?.to_str())
            .unwrap()
            .to_string()
    });

    let config = if let Some(path) = opts.config {
        cfg_loader.load_path(&path)?
    } else {
        cfg_loader.load()?
    };

    let input_timeout = config.input_timeout.map(Duration::from_secs);

    // TODO where are the expose events with depth 32?
    let depth = config.depth;
    debug!("window depth: {}", depth);

    let depth_type = screen
        .allowed_depths
        .iter()
        .find(|d| d.depth == depth)
        .ok_or_else(|| anyhow!("invalid depth"))?;
    let visual_type = depth_type
        .visuals
        .get(0)
        .ok_or_else(|| anyhow!("depth has no visual types"))?;

    let label = if let Some(ref label) = opts.label {
        label
    } else {
        &config.label
    };
    debug!("label {}", label);

    let surface = dialog::XcbSurface::new(&conn, screen.root, depth, visual_type, 1, 1)?;
    let mut dialog = dialog::Dialog::new(config.dialog, &screen, surface, label)?;
    let (window_width, window_height) = dialog.window_size();
    debug!("window width: {}, height: {}", window_width, window_height);

    let colormap = if visual_type.visual_id == screen.root_visual {
        screen.default_colormap
    } else {
        debug!("depth requires a new colormap");
        let colormap = conn
            .generate_id()
            .map_err(|e| conn.xerr_from("generate colormap id", e))?;
        conn.create_colormap(
            xproto::ColormapAlloc::NONE,
            colormap,
            screen.root,
            visual_type.visual_id,
        )?;
        colormap
    };

    let window = conn
        .generate_id()
        .map_err(|e| conn.xerr_from("generate window id", e))?;
    conn.create_window(
        depth,
        window,
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
            .colormap(colormap),
    )?;

    let hostname = gethostname::gethostname();
    let mut title = config.title;
    title.push('@');
    title.push_str(&hostname.to_string_lossy());
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
        atoms._NET_WM_NAME,
        atoms.UTF8_STRING,
        title.as_bytes(),
    )?;
    conn.change_property8(
        xproto::PropMode::REPLACE,
        window,
        xproto::AtomEnum::WM_CLASS,
        xproto::AtomEnum::STRING,
        [instance.as_bytes(), CLASS.as_bytes()]
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
        &[atoms._NET_WM_WINDOW_TYPE_NORMAL],
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
    wm_hints.set(&*conn, window)?;

    // try to prevent resizing
    let size_hints = properties::WmSizeHints {
        size: Some((
            properties::WmSizeHintsSpecification::ProgramSpecified,
            window_width.into(),
            window_height.into(),
        )),
        min_size: Some((window_width.into(), window_height.into())),
        max_size: Some((window_width.into(), window_height.into())),
        ..properties::WmSizeHints::default()
    };
    size_hints.set_normal_hints(&*conn, window)?;

    dialog
        .surface
        .setup_pixmap(window, window_width, window_height)?;
    dialog.init();
    let backbuffer = backbuffer::Backbuffer::new(&conn, window, dialog)?;

    conn.map_window(window)?;
    debug!("init took {}ms", now.elapsed().as_millis());

    let mut xcon = event::XContext {
        keyboard,
        conn: &conn,
        backbuffer,
        window,
        atoms,
        colormap,
        own_colormap: colormap != screen.default_colormap,
        input_timeout,
        width: window_width,
        height: window_height,
        debug: opts.debug,
        grab_keyboard: config.grab_keyboard,
    };

    xcon.run_xevents().await
}

const AFTER_HELP: &str = "\
ENVIRONMENTAL VARIABLES:
    XASKPASS_LOG            Logging level (default 'info'). See https://docs.rs/env_logger for
                            syntax.
    XASKPASS_LOG_STYLE      Print style characters. One of 'auto', 'always', 'never'
                            (default 'auto').
";

#[derive(Clap)]
#[clap(
    version = env!("XASKPASS_BUILD_FULL_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    )]
struct Opts {
    #[clap(long)]
    /// The instance name
    /// (default <executable name>)
    name: Option<String>,

    #[clap(short, long)]
    /// Configuration file path (default: see below)
    config: Option<PathBuf>,

    #[clap(short, long)]
    /// Include timestamps and sensitive information in logs.
    debug: bool,

    /// Label in the dialog.
    label: Option<String>,
}

fn run() -> i32 {
    let now = Instant::now();

    let app = Opts::into_app();
    let cfg_loader = config::Loader::new();
    let help = format!(
        "{}\n\nCONFIGURATION FILE:\n    default: {}{}.toml",
        AFTER_HELP,
        cfg_loader.xdg_dirs.get_config_home().display(),
        config::NAME,
    );
    let app = app.after_help(&*help);
    let opts = Opts::from_arg_matches(&app.get_matches());

    let mut log = env_logger::Builder::from_env(
        env_logger::Env::new()
            .filter_or("XASKPASS_LOG", "info")
            .write_style("XASKPASS_LOG_STYLE"),
    );
    if opts.debug {
        log.format_timestamp_millis();
    } else {
        log.format_timestamp(None).format_module_path(false);
    }
    log.init();

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
            ret = run_xcontext(cfg_loader, opts, now) => {
                match ret {
                    Ok(Some(pass)) => {
                        pass.write_stdout().unwrap();
                        mainret = 0;
                    }
                    Ok(None) => {
                        debug!("cancelled");
                    },
                    Err(err) => {
                        error!("{}", err);
                        let mut src = err.source();
                        while let Some(s) = src {
                            error!("{}", s);
                            src = s.source();
                        }
                        mainret = 2;
                    }
                }
            }
        }
    });
    debug!("exit");
    mainret
}

fn main() {
    // std::process::exit exits without dropping objects so call it in minimal stack
    std::process::exit(run());
}
