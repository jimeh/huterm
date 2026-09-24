//! Shared private X11 event connection, with bounded drains and explicit shutdown.
use super::*;
use crate::quake::observation::{CHANGED, DISPLAY, Signal};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{Read as _, Write as _},
    os::{fd::AsFd as _, unix::net::UnixStream},
    rc::Weak,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use x11rb::protocol::{
    Event,
    randr::{ModeFlag, NotifyMask},
    xproto::ChangeWindowAttributesAux,
};

pub(super) type SharedObserver = Rc<RefCell<Weak<Hub>>>;
#[derive(Clone)]
pub(crate) struct Observer(Rc<Subscription>);
struct Subscription {
    window: Window,
    hub: Rc<Hub>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.send(Command::Remove(self.window.id));
    }
}
pub(super) struct Hub {
    commands: mpsc::Sender<Command>,
    cancel: UnixStream,
    worker: Option<JoinHandle<()>>,
    failed: Arc<AtomicBool>,
}
enum Command {
    Add(u32, Signal, mpsc::Sender<Result<(), String>>),
    Remove(u32),
    Stop,
}
impl Hub {
    fn new() -> anyhow::Result<Rc<Self>> {
        let (commands, receiver) = mpsc::channel();
        let (cancel, cancellation) = UnixStream::pair()?;
        cancel.set_nonblocking(true)?;
        cancellation.set_nonblocking(true)?;
        let failed = Arc::new(AtomicBool::new(false));
        let failure = Arc::clone(&failed);
        let worker = std::thread::Builder::new().name("quake-x11-events".into()).spawn(move || {
            let mut subscribers = HashMap::new();
            if let Err(error) = observe_events(&cancellation, &receiver, &mut subscribers) {
                failure.store(true, Ordering::Release);
                eprintln!("Quake X11 observer failed: {error:#}; retained owners use bounded native-state fallback");
                for signal in subscribers.values() { signal.notify(DISPLAY); }
            }
        })?;
        Ok(Rc::new(Self {
            commands,
            cancel,
            worker: Some(worker),
            failed,
        }))
    }
    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
        let _ = (&self.cancel).write(&[1]);
    }
}
impl Drop for Hub {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Window {
    pub fn observe(
        &self,
        signal: Signal,
        platform: &Platform,
    ) -> anyhow::Result<Observer> {
        let existing = platform
            .observer
            .borrow()
            .upgrade()
            .filter(|hub| !hub.failed.load(Ordering::Acquire));
        let hub = if let Some(hub) = existing {
            hub
        } else {
            let hub = Hub::new()?;
            *platform.observer.borrow_mut() = Rc::downgrade(&hub);
            hub
        };
        let (ready, acknowledgement) = mpsc::channel();
        hub.send(Command::Add(self.id, signal, ready));
        acknowledgement
            .recv()
            .map_err(|_| {
                anyhow::anyhow!("X11 observer stopped during registration")
            })?
            .map_err(anyhow::Error::msg)?;
        Ok(Observer(Rc::new(Subscription {
            window: self.clone(),
            hub,
        })))
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "one bounded event loop owns subscription commands and cancellation ordering"
)]
fn observe_events(
    cancellation: &UnixStream,
    commands: &mpsc::Receiver<Command>,
    subscribers: &mut HashMap<u32, Signal>,
) -> anyhow::Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let atom = |name: &[u8]| -> anyhow::Result<u32> {
        Ok(connection.intern_atom(false, name)?.reply()?.atom)
    };
    let properties = [
        atom(b"_NET_ACTIVE_WINDOW")?,
        atom(b"_NET_CURRENT_DESKTOP")?,
        atom(b"_NET_WORKAREA")?,
        atom(b"_NET_WM_STATE")?,
        atom(b"_NET_WM_DESKTOP")?,
        atom(b"_NET_FRAME_EXTENTS")?,
    ];
    connection
        .change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
    connection
        .randr_select_input(
            root,
            NotifyMask::SCREEN_CHANGE
                | NotifyMask::CRTC_CHANGE
                | NotifyMask::OUTPUT_CHANGE
                | NotifyMask::RESOURCE_CHANGE,
        )?
        .check()?;
    loop {
        // Commands (including cancellation) precede every bounded event batch.
        // A flooding X server cannot keep Drop waiting behind an unbounded drain.
        for command in commands.try_iter() {
            match command {
                Command::Stop => return Ok(()),
                Command::Remove(window) => {
                    subscribers.remove(&window);
                }
                Command::Add(window, signal, ready) => {
                    let result = (|| -> anyhow::Result<()> {
                        connection
                            .change_window_attributes(
                                window,
                                &ChangeWindowAttributesAux::new().event_mask(
                                    EventMask::STRUCTURE_NOTIFY
                                        | EventMask::PROPERTY_CHANGE
                                        | EventMask::FOCUS_CHANGE
                                        | EventMask::VISIBILITY_CHANGE,
                                ),
                            )?
                            .check()?;
                        connection.flush()?;
                        Ok(())
                    })();
                    match result {
                        Ok(()) => {
                            signal.notify(DISPLAY);
                            subscribers.insert(window, signal);
                            let _ = ready.send(Ok(()));
                        }
                        Err(error) => {
                            let _ = ready.send(Err(error.to_string()));
                        }
                    }
                }
            }
        }
        let mut consumed = 0;
        while consumed < 128 {
            let Some(event) = connection.poll_for_event()? else {
                break;
            };
            consumed += 1;
            let (window, flags) = match event {
                Event::PropertyNotify(event)
                    if properties.contains(&event.atom) =>
                {
                    (
                        event.window,
                        if event.atom == properties[0] {
                            CHANGED
                        } else {
                            DISPLAY
                        },
                    )
                }
                Event::ConfigureNotify(event) => (event.window, CHANGED),
                Event::MapNotify(event) => (event.window, CHANGED),
                Event::UnmapNotify(event) => (event.window, CHANGED),
                Event::VisibilityNotify(event) => (event.window, CHANGED),
                Event::FocusIn(event) | Event::FocusOut(event) => {
                    (event.event, CHANGED)
                }
                Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                    (root, DISPLAY)
                }
                Event::DestroyNotify(event) => {
                    subscribers.remove(&event.window);
                    continue;
                }
                _ => continue,
            };
            if window == root {
                for signal in subscribers.values() {
                    signal.notify(flags);
                }
            } else if let Some(signal) = subscribers.get(&window) {
                signal.notify(flags);
            }
        }
        let mut fds = [
            PollFd::new(connection.stream().as_fd(), PollFlags::POLLIN),
            PollFd::new(cancellation.as_fd(), PollFlags::POLLIN),
        ];
        match poll(
            &mut fds,
            if consumed == 128 {
                PollTimeout::ZERO
            } else {
                PollTimeout::NONE
            },
        ) {
            Err(nix::errno::Errno::EINTR) => continue,
            result => {
                result?;
            }
        }
        if fds[1]
            .revents()
            .is_some_and(|flags| flags.contains(PollFlags::POLLIN))
        {
            let _ = (&*cancellation).read(&mut [0; 256]);
        }
        anyhow::ensure!(
            !fds[0].revents().is_some_and(|flags| flags.intersects(
                PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL
            )),
            "X11 observation connection closed"
        );
    }
}
impl Observer {
    pub fn failed(&self) -> bool {
        self.0.hub.failed.load(Ordering::Acquire)
    }
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "shared platform clock contract; X11 uses GPUI frames and paced fallback"
    )]
    pub fn set_clock(
        &self,
        _: Option<&Display>,
        _: u64,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }
    pub fn refresh_period(&self, display: &Display) -> Option<Duration> {
        self.period(display).ok().flatten()
    }
    fn period(&self, display: &Display) -> anyhow::Result<Option<Duration>> {
        let platform = &self.0.window.platform;
        let connection = &platform.connection;
        let monitors = connection
            .randr_get_monitors(platform.root, true)?
            .reply()?;
        let resources = connection
            .randr_get_screen_resources_current(platform.root)?
            .reply()?;
        for monitor in monitors.monitors {
            if connection.get_atom_name(monitor.name)?.reply()?.name
                != display.id.as_bytes()
            {
                continue;
            }
            for output in monitor.outputs {
                let output = connection
                    .randr_get_output_info(output, x11rb::CURRENT_TIME)?
                    .reply()?;
                if output.crtc == 0 {
                    continue;
                }
                let crtc = connection
                    .randr_get_crtc_info(output.crtc, x11rb::CURRENT_TIME)?
                    .reply()?;
                if let Some(mode) =
                    resources.modes.iter().find(|mode| mode.id == crtc.mode)
                {
                    let mut seconds = f64::from(mode.htotal)
                        * f64::from(mode.vtotal)
                        / f64::from(mode.dot_clock);
                    if mode.mode_flags.contains(ModeFlag::INTERLACE) {
                        seconds /= 2.0;
                    }
                    if mode.mode_flags.contains(ModeFlag::DOUBLE_SCAN) {
                        seconds *= 2.0;
                    }
                    return Ok((seconds.is_finite() && seconds > 0.0)
                        .then(|| Duration::from_secs_f64(seconds)));
                }
            }
        }
        Ok(None)
    }
}
