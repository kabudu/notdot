//! Read and invoke accessible controls through the user's AT-SPI bus.
use super::{App, MAX_DEPTH, MAX_NODES, Node, Status};
use crate::clean;
use std::time::{Duration, Instant};
use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    zvariant::OwnedObjectPath,
};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ACTION: &str = "org.a11y.atspi.Action";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const CALL_TIMEOUT: Duration = Duration::from_millis(500);
type Object = (String, OwnedObjectPath);

fn connect() -> Result<Connection, String> {
    let session = Builder::session()
        .map_err(|e| e.to_string())?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let proxy = Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")
        .map_err(|e| e.to_string())?;
    let address: String = proxy.call("GetAddress", &()).map_err(|e| e.to_string())?;
    // AT-SPI on Linux uses a local Unix-domain bus.
    if !address.starts_with("unix:") || address.contains(';') {
        return Err("The accessibility bus is not a local Unix connection.".into());
    }
    Builder::address(address.as_str())
        .map_err(|e| e.to_string())?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

pub(super) fn readiness() -> Status {
    // Readiness may be checked from the desktop's async executor.
    let ready = std::thread::spawn(|| connect().is_ok())
        .join()
        .unwrap_or(false);
    if ready {
        Status::Ready
    } else {
        Status::Unsupported(
            "The Linux accessibility bus is unavailable in this desktop session.".into(),
        )
    }
}

fn registered(conn: &Connection) -> Result<Vec<(App, Object)>, String> {
    let root =
        Proxy::new(conn, "org.a11y.atspi.Registry", ROOT, ACCESSIBLE).map_err(|e| e.to_string())?;
    let entries: Vec<Object> = root.call("GetChildren", &()).map_err(|e| e.to_string())?;
    let dbus = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    let mut pids = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    for entry in entries
        .into_iter()
        .take(128)
        .take_while(|_| Instant::now() < deadline)
    {
        if !entry.0.starts_with(':') {
            continue;
        }
        let Ok(pid) = dbus.call::<_, _, u32>("GetConnectionUnixProcessID", &(&entry.0,)) else {
            continue;
        };
        if pid == std::process::id() || !pids.insert(pid) {
            continue;
        }
        let Ok(path) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        let Some(image) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if image.is_empty()
            || image.len() > 100
            || image.contains(':')
            || image.ends_with(" (deleted)")
        {
            continue;
        }
        let Ok(pid) = i32::try_from(pid) else {
            continue;
        };
        let name = clean(image, 100);
        result.push((
            App {
                bundle_id: format!("linux:{}:{pid}", name.to_ascii_lowercase()),
                name,
                pid,
            },
            entry,
        ));
    }
    Ok(result)
}

pub(super) fn apps() -> Result<Vec<App>, String> {
    Ok(registered(&connect()?)?
        .into_iter()
        .map(|(app, _)| app)
        .collect())
}

struct Walk<'a> {
    conn: &'a Connection,
    bus: &'a str,
    deadline: Instant,
    visited: std::collections::HashSet<OwnedObjectPath>,
    nodes: Vec<Node>,
    target: Option<(&'a str, &'a str, &'a str)>,
    pressed: bool,
}

impl Walk<'_> {
    fn visit(&mut self, object: Object, reference: String, depth: usize) -> Result<(), String> {
        if object.0 != self.bus
            || depth >= MAX_DEPTH
            || self.pressed
            || self.visited.len() >= MAX_NODES
            || Instant::now() >= self.deadline
            || !self.visited.insert(object.1.clone())
        {
            return Ok(());
        }
        let proxy = Proxy::new(self.conn, self.bus, object.1.as_str(), ACCESSIBLE)
            .map_err(|e| e.to_string())?;
        let Ok(role) = proxy.call::<_, _, String>("GetRoleName", &()) else {
            return Ok(());
        };
        if role == "password text" {
            return Ok(());
        }
        if depth > 0 {
            let states: Vec<u32> = proxy.call("GetState", &()).unwrap_or_default();
            let first = states.first().copied().unwrap_or_default();
            // AT-SPI state constants ENABLED=8, SENSITIVE=24, SHOWING=25, VISIBLE=30.
            let visible = (first & ((1 << 25) | (1 << 30))) == ((1 << 25) | (1 << 30));
            if !visible {
                return Ok(());
            }
            let enabled = (first & ((1 << 8) | (1 << 24))) == ((1 << 8) | (1 << 24));
            let label = clean(
                &proxy.get_property::<String>("Name").unwrap_or_default(),
                120,
            );
            let pressable =
                matches!(role.as_str(), "push button" | "button") && enabled && !label.is_empty();
            if let Some((wanted, expected_role, expected_label)) = self.target
                && wanted == reference
            {
                if !pressable || expected_role != role || expected_label != label {
                    return Err("That button changed. Look at the app again.".into());
                }
                let action = Proxy::new(self.conn, self.bus, object.1.as_str(), ACTION)
                    .map_err(|e| e.to_string())?;
                let count: i32 = action.get_property("NActions").map_err(|e| e.to_string())?;
                for index in 0..count.clamp(0, 8) {
                    let name: String = action
                        .call("GetName", &(index,))
                        .map_err(|e| e.to_string())?;
                    if matches!(name.as_str(), "click" | "press" | "activate") {
                        if Instant::now() >= self.deadline {
                            return Err("The app did not respond in time.".into());
                        }
                        self.pressed = action
                            .call("DoAction", &(index,))
                            .map_err(|e| e.to_string())?;
                        return if self.pressed {
                            Ok(())
                        } else {
                            Err("The app refused the button action.".into())
                        };
                    }
                }
                return Err("This button does not offer a supported action.".into());
            }
            self.nodes.push(Node {
                reference: reference.clone(),
                role,
                label,
                value: String::new(),
                pressable,
                disabled: !enabled,
            });
        }
        let count: i32 = proxy.get_property("ChildCount").unwrap_or_default();
        for index in 0..count.clamp(0, MAX_NODES as i32) {
            if self.pressed || self.visited.len() >= MAX_NODES || Instant::now() >= self.deadline {
                break;
            }
            if let Ok(child) = proxy.call("GetChildAtIndex", &(index,)) {
                self.visit(child, format!("{reference}.{index}"), depth + 1)?;
            }
        }
        Ok(())
    }
}

fn walk(pid: i32, target: Option<(&str, &str, &str)>) -> Result<Vec<Node>, String> {
    let conn = connect()?;
    let (_, root) = registered(&conn)?
        .into_iter()
        .find(|(app, _)| app.pid == pid)
        .ok_or("The app has no registered accessible window.")?;
    let bus = root.0.clone();
    let mut walk = Walk {
        conn: &conn,
        bus: &bus,
        deadline: Instant::now() + Duration::from_secs(8),
        visited: Default::default(),
        nodes: Vec::new(),
        target,
        pressed: false,
    };
    walk.visit(root, "0".into(), 0)?;
    if target.is_some() && !walk.pressed {
        return Err("That button is no longer available. Look at the app again.".into());
    }
    Ok(walk.nodes)
}

pub(super) fn observe(pid: i32) -> Result<Vec<Node>, String> {
    walk(pid, None)
}

pub(super) fn press(pid: i32, reference: &str, role: &str, label: &str) -> Result<(), String> {
    walk(pid, Some((reference, role, label))).map(|_| ())
}
