//! Wayland protocol handlers and state. Phase 1 wires up the minimum globals
//! (`wl_compositor`, `wl_shm`, `xdg_wm_base`) via Smithay, following anvil's
//! protocol bootstrapping structurally (see ADR-0004). No window management,
//! layout, or IPC code is carried over — only the protocol-globals plumbing.
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Wayland protocol | Global objects, client connections, protocol state machine | in-proc | main thread (event-loop bound) | Malformed client request → protocol error to that client only, client disconnected, compositor unaffected |

use std::sync::{atomic::AtomicBool, Arc};

use ryowm_common::Rect;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
    desktop::{Space, Window},
    input::{Seat, SeatHandler, SeatState, keyboard::XkbConfig, pointer::PointerHandle},
    reexports::{
        calloop::{Interest, Mode, PostAction, generic::Generic},
        wayland_server::{
            Client,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_surface},
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Clock, Monotonic},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, get_parent},
        output::OutputHandler,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};
use tracing::{info, warn};

/// Per-client state stored by wayland-server. Required by `CompositorHandler`.
#[derive(Debug, Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        // Phase 1 exit criterion: clean disconnect must drop all of the
        // client's Wayland resources without affecting the compositor.
        info!(?client_id, ?reason, "Wayland client disconnected");
    }
}

/// The compositor's Wayland-facing state. Owns all Smithay protocol state
/// objects and the display handle.
///
/// Note: Smithay has its own type also named `CompositorState`; it is kept
/// behind the `smithay_compositor` field and never conflated with our
/// `ryowm-core::state::CompositorState` (the ownership root for windows,
/// workspaces, and outputs).
pub struct RyoWmState {
    pub running: Arc<AtomicBool>,
    pub display_handle: DisplayHandle,

    // Smithay protocol state objects.
    pub smithay_compositor: CompositorState,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    pub seat_state: SeatState<RyoWmState>,
    // Registered with clients via the wl_seat global in `new`, but not read
    // until input focus handling lands (Phase 3).
    #[allow(dead_code)]
    pub seat: Seat<RyoWmState>,
    #[allow(dead_code)]
    pub pointer: PointerHandle<RyoWmState>,

    // Desktop workspace. Phase 2 maps toplevels here for compositing only;
    // window lifecycle/focus stay Phase 4 work (see `window/mod.rs`).
    pub space: Space<Window>,

    /// Damage accumulated since the last presented frame, consumed by the
    /// scene renderer each iteration.
    pub pending_damage: Option<Rect>,
    /// Full-output repaint request (window destroyed, output resized).
    pub damage_all: bool,
    /// Frame presentation clock for client `frame` callbacks.
    pub clock: Clock<Monotonic>,
}

impl RyoWmState {
    pub fn new(
        display: Display<RyoWmState>,
        handle: calloop::LoopHandle<'static, RyoWmState>,
    ) -> Self {
        let dh = display.handle();
        let running = Arc::new(AtomicBool::new(true));

        // Client I/O: dispatch Wayland requests through the Smithay handlers
        // below. `Generic` hands the stored `Display` back as `&mut NoIoDrop`;
        // `get_mut` is unsafe only because it could drop the polled fd, which
        // we never do — same pattern as Smithay's anvil (see ADR-0004).
        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, data: &mut RyoWmState| {
                    unsafe {
                        display.get_mut().dispatch_clients(data).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to register Wayland display with the reactor");

        // Accept clients on an auto-allocated Wayland socket.
        let source =
            ListeningSocketSource::new_auto().expect("Failed to bind Wayland socket");
        let socket_name = source.socket_name().to_string_lossy().into_owned();
        info!(socket = socket_name, "Listening for Wayland clients");

        handle
            .insert_source(source, |client_stream, _, data: &mut RyoWmState| {
                match data.display_handle.insert_client(
                    client_stream,
                    Arc::new(ClientState::default()),
                ) {
                    Ok(_) => info!("Wayland client connected"),
                    Err(err) => warn!("Error adding Wayland client: {}", err),
                }
            })
            .expect("Failed to register Wayland socket with the reactor");

        // Minimum protocol globals: wl_compositor (+ subcompositor), wl_shm,
        // xdg_wm_base. Nothing else is advertised in Phase 1.
        let smithay_compositor = CompositorState::new::<RyoWmState>(&dh);
        let shm_state = ShmState::new::<RyoWmState>(&dh, vec![]);
        let xdg_shell_state = XdgShellState::new::<RyoWmState>(&dh);
        let mut seat_state = SeatState::new();

        let mut seat = seat_state.new_wl_seat(&dh, "seat0".to_string());
        let pointer = seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("Failed to initialize keyboard");

        RyoWmState {
            running,
            display_handle: dh,
            smithay_compositor,
            shm_state,
            xdg_shell_state,
            seat_state,
            seat,
            pointer,
            space: Space::default(),
            pending_damage: None,
            damage_all: false,
            clock: Clock::new(),
        }
    }

    /// Mapped window owning `surface`, following subsurface links to the
    /// toplevel root, if any.
    fn find_window_for_surface(&self, surface: &wl_surface::WlSurface) -> Option<Window> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        self.space
            .elements()
            .find(|window| {
                window.toplevel().map(|toplevel| toplevel.wl_surface()) == Some(&root)
            })
            .cloned()
    }

    fn window_rect(window: &Window) -> Rect {
        let geometry = window.geometry();
        Rect::new(
            geometry.loc.x,
            geometry.loc.y,
            geometry.size.w,
            geometry.size.h,
        )
    }
}

// --- Trait implementations for Smithay dispatch ---

impl CompositorHandler for RyoWmState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.smithay_compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &wl_surface::WlSurface) {
        // Required first: moves the committed buffer into Smithay's renderer
        // surface state. Without this, surface trees stay empty and nothing
        // is ever composited (see `gles::GlesBackend`).
        on_commit_buffer_handler::<RyoWmState>(surface);
        info!(
            surface = surface.id().protocol_id(),
            "Acknowledged client buffer commit"
        );
        // Phase 2: feed the damage tracker. `on_commit` refreshes the
        // window's bounding box from the new buffer first — without it the
        // geometry below stays empty and damage would be lost.
        match self.find_window_for_surface(surface) {
            Some(window) => {
                window.on_commit();
                let region = Self::window_rect(&window);
                if region.is_empty() {
                    self.damage_all = true;
                } else {
                    self.pending_damage = Some(match self.pending_damage {
                        Some(pending) => pending.union(&region),
                        None => region,
                    });
                }
            }
            // Unmapped surface (e.g. popups, which Phase 2 does not composite
            // yet): repaint everything rather than risk a stale frame.
            None => self.damage_all = true,
        }
    }
}

impl BufferHandler for RyoWmState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {
        // Phase 1: no-op. Phase 2 ties this to scene-graph cleanup.
    }
}

impl ShmHandler for RyoWmState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for RyoWmState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Send an initial configure so the client commits a buffer.
        surface.with_pending_state(|state| {
            state.size = Some((800, 600).into());
        });
        surface.send_configure();
        // Phase 2 compositing attachment only: map the window so the renderer
        // picks it up. Placement, focus, and lifecycle stay Phase 4 work —
        // every mapped window stacks at the origin until tiling lands.
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window, (0, 0), false);
        info!("New xdg_toplevel created, mapped for compositing, sent initial configure (800x600)");
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let destroyed = surface.wl_surface().clone();
        // Bind first: a temporary in the `if let` scrutinee would keep the
        // immutable borrow of `space` alive across `unmap_elem` below.
        let removed = self
            .space
            .elements()
            .find(|window| {
                window.toplevel().map(|toplevel| toplevel.wl_surface()) == Some(&destroyed)
            })
            .cloned();
        if let Some(window) = removed {
            self.space.unmap_elem(&window);
            info!("Unmapped destroyed toplevel");
        }
        // The uncovered area needs repainting next frame.
        self.damage_all = true;
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // Phase 1: acknowledged, not managed.
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
        // Phase 1: no popup grab handling.
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        // Phase 1: no popup repositioning.
    }
}

impl SeatHandler for RyoWmState {
    // `WlSurface` already implements all three focus-target traits in
    // Smithay; dedicated focus types arrive with input handling (Phase 3).
    type KeyboardFocus = wl_surface::WlSurface;
    type PointerFocus = wl_surface::WlSurface;
    type TouchFocus = wl_surface::WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<RyoWmState> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {
        // Phase 1: no focus tracking yet.
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
        // Phase 1: no cursor rendering yet.
    }
}

impl OutputHandler for RyoWmState {}

// --- Delegate macros: route protocol messages to the handlers above ---

delegate_compositor!(RyoWmState);
delegate_shm!(RyoWmState);
delegate_xdg_shell!(RyoWmState);
delegate_seat!(RyoWmState);
delegate_output!(RyoWmState);
