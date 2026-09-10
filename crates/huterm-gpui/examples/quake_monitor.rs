//! Add or remove a no-output `RandR` monitor inside the quake smoke's private Xvfb.
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use x11rb::{
        connection::Connection as _,
        protocol::{
            randr::{ConnectionExt as _, MonitorInfo},
            xproto::ConnectionExt as _,
        },
    };
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let name = connection
        .intern_atom(false, b"huterm-smoke-vanishing")?
        .reply()?
        .atom;
    match std::env::args().nth(1).as_deref() {
        Some("add") => {
            let geometry = connection.get_geometry(root)?.reply()?;
            connection
                .randr_set_monitor(
                    root,
                    MonitorInfo {
                        name,
                        primary: false,
                        automatic: false,
                        x: i16::try_from(geometry.width / 2)?,
                        y: 0,
                        width: geometry.width / 2,
                        height: geometry.height,
                        width_in_millimeters: 170,
                        height_in_millimeters: 210,
                        outputs: Vec::new(),
                    },
                )?
                .check()?;
        }
        Some("remove") => {
            connection.randr_delete_monitor(root, name)?.check()?;
        }
        _ => anyhow::bail!("expected add or remove"),
    }
    connection.flush()?;
    for monitor in connection.randr_get_monitors(root, true)?.reply()?.monitors
    {
        let name = String::from_utf8(
            connection.get_atom_name(monitor.name)?.reply()?.name,
        )?;
        println!("{name}: primary={}", monitor.primary);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {}
