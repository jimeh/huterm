//! Native X11 drag source used only by the isolated desktop integration smoke.
#![allow(unsafe_code)]

#[cfg(target_os = "linux")]
mod source {
    use std::ffi::{CString, c_char, c_int, c_long, c_ulong, c_void};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Client {
        kind: c_int,
        serial: c_ulong,
        send: c_int,
        display: *mut c_void,
        window: c_ulong,
        message: c_ulong,
        format: c_int,
        data: [c_long; 5],
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Request {
        kind: c_int,
        serial: c_ulong,
        send: c_int,
        display: *mut c_void,
        owner: c_ulong,
        requestor: c_ulong,
        selection: c_ulong,
        target: c_ulong,
        property: c_ulong,
        time: c_ulong,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SelectionEvent {
        kind: c_int,
        serial: c_ulong,
        send: c_int,
        display: *mut c_void,
        requestor: c_ulong,
        selection: c_ulong,
        target: c_ulong,
        property: c_ulong,
        time: c_ulong,
    }
    #[repr(C)]
    union Event {
        client: Client,
        request: Request,
        selection: SelectionEvent,
        padding: [c_long; 24],
    }
    #[link(name = "libX11.so.6", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        fn XOpenDisplay(name: *const c_char) -> *mut c_void;
        fn XDefaultRootWindow(display: *mut c_void) -> c_ulong;
        fn XCreateSimpleWindow(
            display: *mut c_void,
            parent: c_ulong,
            x: c_int,
            y: c_int,
            width: u32,
            height: u32,
            border: u32,
            border_pixel: c_ulong,
            background: c_ulong,
        ) -> c_ulong;
        fn XInternAtom(
            display: *mut c_void,
            name: *const c_char,
            only: c_int,
        ) -> c_ulong;
        fn XSetSelectionOwner(
            display: *mut c_void,
            selection: c_ulong,
            owner: c_ulong,
            time: c_ulong,
        ) -> c_int;
        fn XChangeProperty(
            display: *mut c_void,
            window: c_ulong,
            property: c_ulong,
            kind: c_ulong,
            format: c_int,
            mode: c_int,
            data: *const u8,
            len: c_int,
        ) -> c_int;
        fn XSendEvent(
            display: *mut c_void,
            window: c_ulong,
            propagate: c_int,
            mask: c_long,
            event: *mut Event,
        ) -> c_int;
        fn XPending(display: *mut c_void) -> c_int;
        fn XNextEvent(display: *mut c_void, event: *mut Event) -> c_int;
        fn XFlush(display: *mut c_void) -> c_int;
        fn XCloseDisplay(display: *mut c_void) -> c_int;
    }
    #[allow(clippy::too_many_lines)]
    pub fn run() -> anyhow::Result<()> {
        let args: Vec<_> = std::env::args().collect();
        anyhow::ensure!(
            args.len() == 4,
            "expected target window, URI-list file, control directory"
        );
        let target: c_ulong = args[1].parse()?;
        let payload = std::fs::read(&args[2])?;
        let directory = PathBuf::from(&args[3]);
        let mode =
            std::fs::read_to_string(directory.join("mode")).unwrap_or_default();
        // SAFETY: This helper owns one Xlib connection and uses repr(C) XEvent
        // layouts on the supported 64-bit Linux targets. XSendEvent copies data.
        unsafe {
            let display = XOpenDisplay(std::ptr::null());
            anyhow::ensure!(!display.is_null(), "XOpenDisplay failed");
            let source = XCreateSimpleWindow(
                display,
                XDefaultRootWindow(display),
                0,
                0,
                1,
                1,
                0,
                0,
                0,
            );
            let atom = |name: &str| {
                XInternAtom(display, CString::new(name).unwrap().as_ptr(), 0)
            };
            let selection = atom("XdndSelection");
            let uri = atom("text/uri-list");
            let copy = atom("XdndActionCopy");
            let send = |name: &str, values: [c_long; 5]| {
                let mut event = Event {
                    client: Client {
                        kind: 33,
                        serial: 0,
                        send: 1,
                        display,
                        window: target,
                        message: atom(name),
                        format: 32,
                        data: values,
                    },
                };
                XSendEvent(display, target, 0, 0, &raw mut event);
                XFlush(display);
            };
            let source_data = c_long::try_from(source)?;
            let uri_data = c_long::try_from(uri)?;
            let copy_data = c_long::try_from(copy)?;
            XSetSelectionOwner(display, selection, source, 0);
            send("XdndEnter", [source_data, 5 << 24, uri_data, 0, 0]);
            send("XdndPosition", [source_data, 0, 0, 0, copy_data]);
            let started = Instant::now();
            let mut request: Option<(Request, Instant)> = None;
            let mut sequence = 0;
            let mut stale: Option<Request> = None;
            loop {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(10),
                    "native XDND source timed out"
                );
                while XPending(display) > 0 {
                    let mut event = Event { padding: [0; 24] };
                    XNextEvent(display, &raw mut event);
                    if event.client.kind == 30 {
                        let incoming = event.request;
                        if mode == "stale" && stale.is_none() {
                            stale = Some(incoming);
                            send("XdndLeave", [source_data, 0, 0, 0, 0]);
                            send(
                                "XdndEnter",
                                [source_data, 5 << 24, uri_data, 0, 0],
                            );
                            send(
                                "XdndPosition",
                                [source_data, 0, 0, 0, copy_data],
                            );
                        } else {
                            if let Some(old) = stale {
                                let mut notification = Event {
                                    selection: SelectionEvent {
                                        kind: 31,
                                        serial: 0,
                                        send: 1,
                                        display,
                                        requestor: old.requestor,
                                        selection: old.selection,
                                        target: old.target,
                                        property: old.property,
                                        time: old.time,
                                    },
                                };
                                XSendEvent(
                                    display,
                                    target,
                                    0,
                                    0,
                                    &raw mut notification,
                                );
                                XFlush(display);
                                std::fs::write(
                                    directory.join("stale-sent"),
                                    "sent",
                                )?;
                            }
                            request = Some((incoming, Instant::now()));
                            if mode == "early" {
                                send("XdndDrop", [source_data, 0, 0, 0, 0]);
                            }
                        }
                        std::fs::write(
                            directory.join("requested"),
                            "requested",
                        )?;
                    } else if event.client.kind == 33
                        && event.client.message == atom("XdndFinished")
                    {
                        std::fs::write(
                            directory.join("finished"),
                            format!("{}", event.client.data[1] & 1),
                        )?;
                        XCloseDisplay(display);
                        return Ok(());
                    }
                }
                if let Some((pending, created)) = request
                    && created.elapsed() > Duration::from_millis(150)
                {
                    for (index, chunk) in payload.chunks(65536).enumerate() {
                        XChangeProperty(
                            display,
                            pending.requestor,
                            pending.property,
                            uri,
                            8,
                            if index == 0 { 0 } else { 2 },
                            chunk.as_ptr(),
                            c_int::try_from(chunk.len())?,
                        );
                    }
                    let mut event = Event {
                        selection: SelectionEvent {
                            kind: 31,
                            serial: 0,
                            send: 1,
                            display,
                            requestor: pending.requestor,
                            selection: pending.selection,
                            target: pending.target,
                            property: pending.property,
                            time: pending.time,
                        },
                    };
                    XSendEvent(
                        display,
                        pending.requestor,
                        0,
                        0,
                        &raw mut event,
                    );
                    XFlush(display);
                    request = None;
                    std::fs::write(directory.join("ready"), "ready")?;
                }
                if let Ok(action) = std::fs::read_to_string(
                    directory.join(format!("action-{sequence}")),
                ) {
                    match action.as_str() {
                        "move" => send(
                            "XdndPosition",
                            [source_data, 0, 0, 0, copy_data],
                        ),
                        "drop" => {
                            send("XdndDrop", [source_data, 0, 0, 0, 0]);
                        }
                        "exit" => {
                            send("XdndLeave", [source_data, 0, 0, 0, 0]);
                            std::fs::write(
                                directory.join(format!("done-{sequence}")),
                                "done",
                            )?;
                            XCloseDisplay(display);
                            return Ok(());
                        }
                        _ => anyhow::bail!("unknown native XDND action"),
                    }
                    std::fs::write(
                        directory.join(format!("done-{sequence}")),
                        "done",
                    )?;
                    sequence += 1;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    source::run()
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("XDND source requires Linux");
}
