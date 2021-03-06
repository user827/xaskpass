use std::convert::TryInto as _;
use std::error::Error as _;
use std::ops::Deref;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use clap::{Clap, FromArgMatches as _, IntoApp as _};
use log::{debug, error, info};
use tokio::io::unix::AsyncFd;
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::Instant;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::render;
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

use errors::{Result, X11ErrorString as _};
use secret::Passphrase;

pub const CLASS: &str = "SshAskpass";

include!(concat!(env!("XASKPASS_BUILD_HEADER_DIR"), "/icon.rs"));

// A collection of the atoms we will need.
atom_manager! {
    pub AtomCollection: AtomCollectionCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_ICON,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        UTF8_STRING,
        CLIPBOARD,
        XSEL_DATA,
        INCR,
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

impl Deref for Connection {
    type Target = XCBConnection;
    fn deref(&self) -> &Self::Target {
        self.xfd.get_ref()
    }
}

fn create_input_cursor(
    conn: &Connection,
    screen: &xproto::Screen,
    dialog: &dialog::Dialog,
    window: xproto::Window,
) -> Result<XId> {
    //let render_version = render::query_version(conn.xfd.get_ref(), 0, 8)?.reply().map_xerr(conn);
    let pict_format = render::query_pict_formats(conn.deref())?
        .reply()
        .map_xerr(conn)?
        .formats
        .iter()
        .find_map(|format| {
            if format.type_ == render::PictType::DIRECT
                && format.depth == 32
                && format.direct.red_shift == 16
                && format.direct.red_mask == 0xff
                && format.direct.green_shift == 8
                && format.direct.green_mask == 0xff
                && format.direct.blue_shift == 0
                && format.direct.blue_mask == 0xff
                && format.direct.alpha_shift == 24
                && format.direct.alpha_mask == 0xff
            {
                Some(format.id)
            } else {
                None
            }
        })
        .expect("The X11 server is missing the RENDER ARGB_32 standard format!");

    let depth_type = screen
        .allowed_depths
        .iter()
        .find(|d| d.depth == 32)
        .expect("invalid depth");
    let visual_type = depth_type
        .visuals
        .get(0)
        .expect("depth has no visual types");

    let width: u16 = dialog.cursor_size.unwrap().0.try_into().unwrap();
    let height: u16 = dialog.cursor_size.unwrap().1.try_into().unwrap();
    let surface = backbuffer::XcbSurface::new(conn, window, 32, visual_type, width, height)?;
    let cr = cairo::Context::new(&surface);
    let (hot_x, hot_y) = dialog.paint_input_cursor(&cr, width, height);
    surface.flush();
    let picture = conn.generate_id().map_xerr(&conn)?;
    render::create_picture(
        conn.deref(),
        picture,
        surface.pixmap,
        pict_format,
        &Default::default(),
    )?;
    let cursor = conn.generate_id().map_xerr(&conn)?;
    render::create_cursor(conn.deref(), cursor, picture, hot_x, hot_y)?;
    render::free_picture(conn.deref(), picture)?;
    Ok(cursor)
}

async fn run_xcontext(
    cfg_loader: config::Loader,
    opts: Opts,
    startup_time: Instant,
) -> Result<Option<Passphrase>> {
    let (conn, screen_num) = Connection::new()?;
    debug!("connected X server");
    let atoms = AtomCollection::new(&*conn)?;
    debug!("loaded atoms");

    conn.prefetch_extension_information(x11rb::protocol::present::X11_EXTENSION_NAME)?;

    conn.flush()?;

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

    debug!("load config");
    let config = if let Some(path) = opts.config {
        cfg_loader.load_path(&path)?
    } else {
        cfg_loader.load()?
    };
    debug!("config loaded");

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

    let surface = backbuffer::XcbSurface::new(&conn, screen.root, depth, visual_type, 1, 1)?;
    let backbuffer = backbuffer::Backbuffer::new(&conn, screen.root, surface)?;
    conn.flush()?;
    let mut dialog = dialog::Dialog::new(
        config.dialog,
        &screen,
        // TODO should be private
        &backbuffer.cr,
        opts.label.as_deref(),
        opts.debug,
    )?;
    let (window_width, window_height) = dialog.window_size(&backbuffer.cr);
    debug!("window width: {}, height: {}", window_width, window_height);

    let colormap = if visual_type.visual_id == screen.root_visual {
        screen.default_colormap
    } else {
        debug!("depth requires a new colormap");
        let colormap = conn.generate_id().map_xerr(&conn)?;
        conn.create_colormap(
            xproto::ColormapAlloc::NONE,
            colormap,
            screen.root,
            visual_type.visual_id,
        )?;
        colormap
    };

    let window = conn.generate_id().map_xerr(&conn)?;
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

    let atoms = atoms.reply().map_xerr(&conn)?;

    let hostname = if let Some(hn) = std::env::var_os("HOSTNAME") {
        hn
    } else {
        gethostname::gethostname()
    };
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

    debug!("map window");
    conn.map_window(window)?;
    debug!("flush");
    conn.flush()?;

    // Load the slow ones after we have mapped the window
    debug!("dialog init");
    let mut backbuffer = backbuffer.reply()?;
    backbuffer.init(window, &mut dialog)?;

    debug!("keyboard init");
    let keyboard = keyboard::Keyboard::new(&conn)?;

    let input_cursor = if dialog.cursor_size.is_some()
        && conn
            .extension_information(render::X11_EXTENSION_NAME)?
            .is_some()
    {
        debug!("cursor init");
        Some(create_input_cursor(&conn, &screen, &dialog, window)?)
    } else {
        None
    };
    debug!("init took {}ms", startup_time.elapsed().as_millis());

    let mut xcon = event::XContext {
        keyboard,
        conn: &conn,
        backbuffer,
        window,
        atoms,
        colormap,
        own_colormap: colormap != screen.default_colormap,
        width: window_width,
        height: window_height,
        grab_keyboard: config.grab_keyboard,
        startup_time,
        first_expose_received: false,
        keyboard_grabbed: false,
        input_cursor,
    };

    dialog.run_events(&mut xcon).await
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

    /// Generate default config into path. Consider using the supplied default
    /// configuration file with comments instead.
    #[clap(long, hidden = true)]
    gen_config: Option<PathBuf>,
}

fn run() -> i32 {
    let startup_time = Instant::now();

    let app = Opts::into_app();
    let cfg_loader = config::Loader::new();
    let help = format!(
        "{}\nCONFIGURATION FILE:\n    default: {}{}.toml",
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

    match run_logged(cfg_loader, opts, startup_time) {
        Ok(ret) => ret,
        Err(err) => {
            error!("{}", err);
            let mut src = err.source();
            while let Some(s) = src {
                error!("{}", s);
                src = s.source();
            }
            2
        }
    }
}

fn run_logged(cfg_loader: config::Loader, opts: Opts, startup_time: Instant) -> Result<i32> {
    if let Some(path) = opts.gen_config {
        let cfg = config::Config::default();
        cfg_loader.save_path(&path, &cfg)?;
        return Ok(0);
    }

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
            ret = run_xcontext(cfg_loader, opts, startup_time) => {
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
