//! PP-25 / D-5 / G-4: a default install talks only to loopback and the user's allow-list.
//!
//! Behavioral: the daemon (with its sidecar, Chromium and every child) runs in a private
//! network namespace whose default routes point back at loopback and whose resolver points at
//! an unreachable fake address, so nothing it sends leaves the machine. A packet capture on
//! that loopback records the destination of every IPv4/IPv6 TCP, UDP and ICMP packet, however
//! short-lived (fast-failing connects and unconnected UDP included); a `/proc/net` sampler
//! adds the owning process for diagnostics. Every destination seen during a full scripted
//! session, a routine run and memory use must pass `openside_policy::Egress`.
//!
//! Controls so the test cannot pass vacuously: the observer test proves the capture records
//! long-lived TCP, fast-failing TCP, short-lived UDP and DNS attempts and does not invent
//! others; the main test proves an allow-listed peer inside the namespace is really reached
//! through the sidecar's enforcing transport while an unlisted host and a redirect hop to
//! one are refused before any connection, and that the browser and REPL steps succeeded.
//!
//! Limits: the capture sees what is sent through the namespace's network stack; it is not a
//! proof about code paths this suite does not drive (for example live LLM provider traffic).
//! Structural: no telemetry SDK is in the Rust or Python dependency graph. That is a blacklist
//! of known packages, complementary to the network observation, not proof of absence.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use openside_e2e::Harness;
use openside_policy::Egress;
use parking_lot::Mutex;
use serde_json::{json, Value};

/// Documentation-only address (RFC 5737) used as the fake resolver.
const FAKE_RESOLVER: &str = "192.0.2.53";
/// User-configured destination for the allow-list half of the assertion. Every address is
/// routed back to the namespace's loopback, so a wildcard listener on this port is this peer.
const ALLOWED_PEER: &str = "198.51.100.7:18443";
/// Never allow-listed: the sidecar must refuse it before any connection is made.
const UNLISTED_HOST: &str = "192.0.2.77:18443";
/// Where the allowed peer redirects; also never allow-listed.
const REDIRECT_HOP: &str = "192.0.2.88:18443";
const SITE_PORT: u16 = 18_431;

/// Records the destination of every packet on the namespace's loopback until `stop` exists.
/// Raw capture needs CAP_NET_RAW, which the namespace wrapper grants.
const SNIFFER: &str = r#"
import json, os, socket, sys, time
ready, stop, out = sys.argv[1:4]
# Detached (double fork, own session): neither killing the daemon's process group nor the
# harness's descendant sweep may take the capture down before it reports.
if os.fork():
    os._exit(0)
os.setsid()
if os.fork():
    os._exit(0)
sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3))
sock.bind(("lo", 0))
sock.settimeout(0.02)
open(ready, "w").close()
seen = set()
next_check = 0.0
deadline = time.monotonic() + 900
while True:
    now = time.monotonic()
    if now >= next_check:
        # Bounded lifetime: a test that panicked before reporting must not leak the capture.
        if os.path.exists(stop) or now > deadline:
            break
        next_check = now + 0.05
    try:
        frame = sock.recv(65535)
    except socket.timeout:
        continue
    kind = frame[12:14]
    if kind == b"\x08\x00" and len(frame) >= 34:
        header = (frame[14] & 15) * 4
        proto = frame[23]
        sport = dport = 0
        if proto in (6, 17) and len(frame) >= 14 + header + 4:
            sport = int.from_bytes(frame[14 + header:14 + header + 2], "big")
            dport = int.from_bytes(frame[14 + header + 2:14 + header + 4], "big")
        seen.add((proto, socket.inet_ntop(socket.AF_INET, frame[30:34]), dport,
                  socket.inet_ntop(socket.AF_INET, frame[26:30]), sport))
    elif kind == b"\x86\xdd" and len(frame) >= 54:
        proto = frame[20]
        sport = dport = 0
        if proto in (6, 17) and len(frame) >= 58:
            sport = int.from_bytes(frame[54:56], "big")
            dport = int.from_bytes(frame[56:58], "big")
        seen.add((proto, socket.inet_ntop(socket.AF_INET6, frame[38:54]), dport,
                  socket.inet_ntop(socket.AF_INET6, frame[22:38]), sport))
with open(out + ".tmp", "w") as handle:
    for proto, dst, dport, src, sport in sorted(seen):
        handle.write(json.dumps({"proto": proto, "dst": dst, "port": dport, "src": src, "sport": sport}) + "\n")
os.replace(out + ".tmp", out)
"#;

/// A fixture site on loopback plus the allow-listed peer on a wildcard port. The peer logs
/// every request (Host and path) so the test can see what actually reached it.
const FIXTURE: &str = r#"
import http.server, json, sys, threading
workspace, site_port, peer_port, ready, log = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]

class Site(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=workspace, **kwargs)
    def log_message(self, *args):
        pass

class Peer(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as handle:
            handle.write(json.dumps({"host": self.headers.get("Host"), "path": self.path}) + "\n")
        if self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "http://%s/hop" % sys.argv[6])
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = b"peer-ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

site = http.server.ThreadingHTTPServer(("127.0.0.1", site_port), Site)
peer = http.server.ThreadingHTTPServer(("0.0.0.0", peer_port), Peer)
threading.Thread(target=site.serve_forever, daemon=True).start()
open(ready, "w").close()
peer.serve_forever()
"#;

/// Runs the sidecar's own enforcement module (the code the daemon's sidecar installs at
/// session open with `network.allow`) against the namespace's peer.
const CLIENT: &str = r#"
import json, sys
import httpx
from openside_agent.providers import egress

peer, unlisted = sys.argv[1], sys.argv[2]
egress.install([peer])
result = {"reached": httpx.get("http://%s/" % peer, timeout=5).text}
for key, url in (("unlisted", "http://%s/" % unlisted), ("redirect", "http://%s/redirect" % peer)):
    try:
        httpx.get(url, timeout=5, follow_redirects=True)
        result[key] = "reached"
    except egress.EgressRefused:
        result[key] = "refused"
print(json.dumps(result))
"#;

fn proc_address(text: &str, v6: bool) -> Option<SocketAddr> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if v6 {
        let mut bytes = [0u8; 16];
        for (word, chunk) in address.as_bytes().chunks(8).enumerate() {
            let value = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            bytes[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        IpAddr::V6(Ipv6Addr::from(bytes))
    } else {
        let value = u32::from_str_radix(address, 16).ok()?;
        IpAddr::V4(Ipv4Addr::from(value.to_le_bytes()))
    };
    Some(SocketAddr::new(ip, port))
}

/// Remote endpoint and socket inode of every connected socket in the namespace of `pid`,
/// except the allowed peer's own side of a connection (its replies are not new destinations).
fn remotes(pid: u32) -> Vec<(SocketAddr, u64)> {
    let allowed: SocketAddr = ALLOWED_PEER.parse().unwrap();
    let mut found = Vec::new();
    for (file, v6) in [
        ("tcp", false),
        ("udp", false),
        ("tcp6", true),
        ("udp6", true),
    ] {
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{file}")) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let mut columns = line.split_whitespace();
            let (Some(local), Some(remote)) = (columns.nth(1), columns.next()) else {
                continue;
            };
            let (Some(local), Some(remote)) = (proc_address(local, v6), proc_address(remote, v6))
            else {
                continue;
            };
            let inode = line
                .split_whitespace()
                .nth(9)
                .and_then(|i| i.parse().ok())
                .unwrap_or(0);
            if !remote.ip().is_unspecified() && local != allowed {
                found.push((remote, inode));
            }
        }
    }
    found
}

/// Command line of the process holding socket `inode`, for failure messages.
fn owner(inode: u64) -> String {
    let needle = format!("socket:[{inode}]");
    for process in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|link| link.as_os_str() == needle.as_str()) {
                let cmdline = std::fs::read(process.path().join("cmdline")).unwrap_or_default();
                return String::from_utf8_lossy(&cmdline).replace('\0', " ");
            }
        }
    }
    "exited".into()
}

type Seen = Arc<Mutex<BTreeMap<SocketAddr, String>>>;

/// `netns` is the namespace link of `pid` when sampling began: once the daemon has exited,
/// its pid can be reused by a host process, whose sockets are not the namespace's. The link
/// is checked after the read, so a table read from a reused pid is always discarded.
fn sample(pid: u32, netns: &Path, seen: &Seen) {
    let found = remotes(pid);
    if std::fs::read_link(format!("/proc/{pid}/ns/net"))
        .ok()
        .as_deref()
        != Some(netns)
    {
        return;
    }
    for (peer, inode) in found {
        seen.lock().entry(peer).or_insert_with(|| {
            if peer.ip().is_loopback() {
                String::new()
            } else {
                owner(inode)
            }
        });
    }
}

/// Merges the sampler (long-lived sockets, with their owning process) and the packet capture
/// (everything, however short-lived) into one destination -> description map.
struct Observer {
    seen: Seen,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    dir: PathBuf,
}

impl Observer {
    /// The wrapper blocks on `netns.go` until the first sample, and starts the packet capture
    /// before it publishes its pid, so nothing the daemon does at startup is missed.
    fn start(dir: &Path) -> Self {
        let seen = Seen::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (pid_file, go) = (dir.join("netns.pid"), dir.join("netns.go"));
        let thread = {
            let (seen, stop) = (seen.clone(), stop.clone());
            std::thread::spawn(move || {
                let pid = loop {
                    if let Some(pid) = std::fs::read_to_string(&pid_file)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok())
                    {
                        break pid;
                    }
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                };
                // The wrapper blocks on `netns.go`, so the namespace's process is alive here.
                let netns = std::fs::read_link(format!("/proc/{pid}/ns/net")).unwrap();
                sample(pid, &netns, &seen);
                std::fs::write(&go, "").unwrap();
                while !stop.load(Ordering::SeqCst) {
                    sample(pid, &netns, &seen);
                    std::thread::sleep(Duration::from_millis(5));
                }
                sample(pid, &netns, &seen);
            })
        };
        Self {
            seen,
            stop,
            thread: Some(thread),
            dir: dir.to_owned(),
        }
    }

    fn finish(mut self) -> BTreeMap<SocketAddr, String> {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
        let mut seen = self.seen.lock().clone();
        std::fs::write(self.dir.join("netns.stop"), "").unwrap();
        let capture = self.dir.join("netns.capture.jsonl");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !capture.exists() {
            assert!(
                Instant::now() < deadline,
                "the packet capture never reported"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        for line in std::fs::read_to_string(&capture).unwrap().lines() {
            let record: Value = serde_json::from_str(line).unwrap();
            let label = match record["proto"].as_u64().unwrap() {
                6 => "tcp",
                17 => "udp",
                1 => "icmp",
                // ICMPv6 on the loopback is neighbour/router chatter of the kernel itself.
                _ => continue,
            };
            let source = SocketAddr::new(
                record["src"].as_str().unwrap().parse().unwrap(),
                u16::try_from(record["sport"].as_u64().unwrap()).unwrap(),
            );
            // The allow-listed peer answering a connection is not a new destination.
            if source == ALLOWED_PEER.parse::<SocketAddr>().unwrap() {
                continue;
            }
            let ip: IpAddr = record["dst"].as_str().unwrap().parse().unwrap();
            let port = u16::try_from(record["port"].as_u64().unwrap()).unwrap();
            seen.entry(SocketAddr::new(ip, port))
                .or_insert_with(|| format!("{label} packet (capture)"));
        }
        seen
    }
}

/// bwrap arguments for a private network namespace with loopback-only default routes and a
/// resolver that cannot answer. Host resolver sockets (systemd-resolved, nscd, D-Bus) are
/// hidden so name lookups cannot bypass the namespace through a Unix socket.
fn netns_prefix(dir: &Path) -> Option<String> {
    for tool in ["bwrap", "ip", "nsenter", "python3"] {
        if openside_config::find_on_path(tool).is_none() {
            eprintln!("SKIP: network observation needs {tool} on PATH");
            return None;
        }
    }
    let resolv = dir.join("resolv.conf");
    std::fs::write(&resolv, format!("nameserver {FAKE_RESOLVER}\n")).unwrap();
    std::fs::write(dir.join("sniffer.py"), SNIFFER).unwrap();
    let mut args = vec![
        "bwrap".to_owned(),
        "--unshare-user".into(),
        "--unshare-net".into(),
        "--cap-add".into(),
        "CAP_NET_ADMIN".into(),
        "--cap-add".into(),
        "CAP_NET_RAW".into(),
        "--dev-bind".into(),
        "/".into(),
        "/".into(),
    ];
    let target = std::fs::canonicalize("/etc/resolv.conf")
        .unwrap_or_else(|_| PathBuf::from("/etc/resolv.conf"));
    let mut hidden: Vec<PathBuf> = [
        "/run/systemd/resolve",
        "/run/nscd",
        "/var/run/nscd",
        "/run/dbus",
    ]
    .iter()
    .map(PathBuf::from)
    .filter(|path| path.is_dir())
    .collect();
    if target != Path::new("/etc/resolv.conf") {
        if let Some(parent) = target.parent() {
            if !hidden.iter().any(|path| path == parent) {
                hidden.push(parent.to_owned());
            }
        }
    }
    for path in hidden {
        args.extend(["--tmpfs".into(), path.display().to_string()]);
    }
    args.extend([
        "--ro-bind".into(),
        resolv.display().to_string(),
        target.display().to_string(),
    ]);
    let quoted: Vec<String> = args.iter().map(|arg| format!("'{arg}'")).collect();
    // The probe runs the real capture once (its stop file already exists) so a host that
    // cannot open a raw socket in the namespace skips instead of failing later.
    std::fs::write(dir.join("probe.stop"), "").unwrap();
    let probe = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} -- sh -c \"ip route add default dev lo && python3 '{sniffer}' '{ready}' '{stop}' '{out}'\"",
            quoted.join(" "),
            sniffer = dir.join("sniffer.py").display(),
            ready = dir.join("probe.ready").display(),
            stop = dir.join("probe.stop").display(),
            out = dir.join("probe.out").display(),
        ))
        .output()
        .unwrap();
    // The capture detaches itself, so its report may land just after the probe shell exits.
    let reported = (0..250).any(|_| {
        let found = dir.join("probe.out").exists();
        if !found {
            std::thread::sleep(Duration::from_millis(20));
        }
        found
    });
    if !probe.status.success() || !reported {
        eprintln!(
            "SKIP: private network namespaces with packet capture are unavailable: {}",
            String::from_utf8_lossy(&probe.stderr).trim()
        );
        return None;
    }
    Some(quoted.join(" "))
}

/// Shell wrapper that runs `program "$@"` inside the namespace once the packet capture is
/// recording and the observer has taken its first sample.
fn wrapper(dir: &Path, prefix: &str, program: &str) -> PathBuf {
    let python = openside_config::find_on_path("python3").unwrap();
    let (pid, go) = (dir.join("netns.pid"), dir.join("netns.go"));
    let (ready, stop, out) = (
        dir.join("netns.capture.ready"),
        dir.join("netns.stop"),
        dir.join("netns.capture.jsonl"),
    );
    let inner = format!(
        // The peer address is local to the namespace: a packet from the loopback source
        // address to a foreign destination would be dropped as a martian, so without this
        // the peer could not answer. Every other address still goes nowhere.
        "ip route add default dev lo || exit 97; ip addr add {peer_ip}/32 dev lo || exit 96; ip -6 route add default dev lo 2>/dev/null; \
         '{python}' '{sniffer}' '{ready}' '{stop}' '{out}' & \
         tries=0; while [ ! -e '{ready}' ]; do tries=$((tries+1)); [ $tries -gt 1500 ] && exit 98; sleep 0.01; done; \
         echo $$ > '{pid}'; while [ ! -e '{go}' ]; do sleep 0.01; done; exec \"$0\" \"$@\"",
        peer_ip = ALLOWED_PEER.rsplit_once(':').unwrap().0,
        python = python.display(),
        sniffer = dir.join("sniffer.py").display(),
        ready = ready.display(),
        stop = stop.display(),
        out = out.display(),
        pid = pid.display(),
        go = go.display(),
    );
    let script = dir.join("netns-wrapper.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nexec {prefix} -- sh -c '{}' '{program}' \"$@\"\n",
            inner.replace('\'', "'\\''")
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn violations(
    seen: &BTreeMap<SocketAddr, String>,
    egress: &Egress,
) -> BTreeMap<SocketAddr, String> {
    seen.iter()
        .filter(|(peer, _)| egress.check_ip(peer.ip(), peer.port()).is_err())
        .map(|(peer, owner)| (*peer, owner.clone()))
        .collect()
}

/// Positive and negative controls for the observer itself: every kind of attempt it must
/// catch is caught, including ones too short-lived for a `/proc` sampler, and a destination
/// nobody contacted does not appear.
#[test]
fn observer_sees_dns_tcp_and_short_lived_udp_attempts_leaving_the_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let Some(prefix) = netns_prefix(dir.path()) else {
        return;
    };
    let probe = dir.path().join("probe.sh");
    std::fs::write(
        &probe,
        "#!/bin/sh\n\
         (timeout 2 getent hosts openside-probe.example &)\n\
         # hangs or is refused depending on the kernel: either way a SYN leaves\n\
         (timeout 2 bash -c 'exec 3<>/dev/tcp/192.0.2.1/443' &)\n\
         # fast-failing connect: refused at once, the socket is gone before any sampler runs\n\
         (timeout 2 bash -c 'exec 3<>/dev/tcp/198.51.100.99/1' &)\n\
         # unconnected-style short-lived UDP: one datagram, socket closed immediately\n\
         (timeout 2 bash -c 'exec 3<>/dev/udp/192.0.2.9/9; echo x >&3' &)\n\
         sleep 0.5\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let script = wrapper(dir.path(), &prefix, probe.to_str().unwrap());
    let observer = Observer::start(dir.path());
    let status = std::process::Command::new(&script).status().unwrap();
    let seen = observer.finish();
    assert!(status.success());
    let egress = Egress::new(Vec::<String>::new()).unwrap();
    let blocked = violations(&seen, &egress);
    for (what, peer) in [
        ("TCP attempt", "192.0.2.1:443"),
        ("fast-failing TCP attempt", "198.51.100.99:1"),
        ("short-lived UDP datagram", "192.0.2.9:9"),
        ("DNS query", format!("{FAKE_RESOLVER}:53").as_str()),
    ] {
        assert!(
            blocked.contains_key(&peer.parse().unwrap()),
            "{what} to {peer} not observed: {seen:?}"
        );
    }
    assert!(
        !seen.contains_key(&"192.0.2.250:443".parse().unwrap()),
        "the observer reported a destination nobody contacted: {seen:?}"
    );
    // Allow-listing a destination removes exactly that one from the violations.
    let allowed = Egress::new(vec!["192.0.2.9:9".to_owned()]).unwrap();
    let remaining = violations(&seen, &allowed);
    assert!(!remaining.contains_key(&"192.0.2.9:9".parse().unwrap()));
    assert!(remaining.contains_key(&"192.0.2.1:443".parse().unwrap()));
}

/// Waits for `path`, failing fast with the child's stderr if it exits first.
fn wait_ready(path: &Path, child: &mut std::process::Child, stderr: &Path, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "{what} exited early ({status}): {}",
                std::fs::read_to_string(stderr).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "{what} never became ready");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A command that runs inside the user and network namespaces of the daemon (`pid`).
fn in_namespace(pid: u32, program: &Path, stderr: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("nsenter");
    command
        .arg(format!("--user=/proc/{pid}/ns/user"))
        .arg(format!("--net=/proc/{pid}/ns/net"))
        .arg("--preserve-credentials")
        .arg(program)
        .env("NO_PROXY", "*")
        .env("no_proxy", "*")
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(stderr).unwrap());
    command
}

async fn settle(harness: &Harness, session: &str, final_text: &str) -> Vec<openside_proto::Event> {
    let mut stream = harness.events(session, 0).await;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let events = stream.settled().await;
            if events.iter().any(|event| {
                matches!(&event.kind, openside_proto::EventKind::AssistantFinal { text, .. } if text == final_text)
            }) {
                return events;
            }
        }
    })
    .await
    .expect("session did not finish")
}

/// The successful result of tool call `id`; a failed or missing call fails the test.
fn tool_result<'a>(events: &'a [openside_proto::Event], id: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            openside_proto::EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error,
            } if tool_call_id == id => {
                assert!(!is_error, "tool call {id} failed: {result}");
                Some(result)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("tool call {id} never ended"))
}

#[tokio::test]
async fn default_install_contacts_only_loopback_and_allow_listed_peers() {
    let dir = tempfile::tempdir().unwrap();
    let Some(prefix) = netns_prefix(dir.path()) else {
        return;
    };
    let python = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent/.venv/bin/python");
    if !python.exists() {
        eprintln!(
            "SKIP: the sidecar virtualenv is missing ({})",
            python.display()
        );
        return;
    }
    let browser = openside_config::discover_browser(&openside_config::Config::default()).ok();
    let mut steps = vec![
        json!({"parts":[{"type":"tool_call","name":"write","args":{"path":"notes.txt","content":"egress"},"id":"write"}]}),
        json!({"parts":[{"type":"tool_call","name":"read","args":{"path":"notes.txt"},"id":"read"}]}),
    ];
    if browser.is_some() {
        steps.push(json!({"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("http://127.0.0.1:{SITE_PORT}/")},"id":"open"}]}));
        steps.push(json!({"parts":[{"type":"tool_call","name":"browser_repl","args":{"code":"await page.title()"},"id":"repl"}]}));
    } else {
        eprintln!("SKIP (browser part): Chromium is missing; only the daemon, sidecar and routine are observed");
    }
    steps.push(json!({"parts":[{"type":"text","text":"egress done"}]}));
    let script = wrapper(dir.path(), &prefix, env!("CARGO_BIN_EXE_e2e-opensided"));
    let Some(mut harness) = Harness::prepare(script.to_str().unwrap(), &json!({"responses":steps}))
    else {
        return;
    };
    std::fs::write(
        harness.workspace().join("index.html"),
        "<!doctype html><title>egress fixture</title>",
    )
    .unwrap();
    let mut config = format!("[network]\nallow = [\"{ALLOWED_PEER}\"]\n");
    if let Some(executable) = &browser {
        config.push_str(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    }
    harness.append_config(&config);
    let observer = Observer::start(dir.path());
    harness.launch().await;
    let pid: u32 = std::fs::read_to_string(dir.path().join("netns.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // Loopback site for the browser plus the allow-listed peer, both inside the namespace.
    let (ready, peer_log) = (
        dir.path().join("fixture.ready"),
        dir.path().join("peer.log"),
    );
    let fixture_script = dir.path().join("fixture.py");
    std::fs::write(&fixture_script, FIXTURE).unwrap();
    let fixture_err = dir.path().join("fixture.err");
    let mut fixture = in_namespace(
        pid,
        &openside_config::find_on_path("python3").unwrap(),
        &fixture_err,
    )
    .arg(&fixture_script)
    .arg(harness.workspace())
    .arg(SITE_PORT.to_string())
    .arg(ALLOWED_PEER.rsplit_once(':').unwrap().1)
    .arg(&ready)
    .arg(&peer_log)
    .arg(REDIRECT_HOP)
    .spawn()
    .expect("nsenter fixture");
    wait_ready(&ready, &mut fixture, &fixture_err, "fixture");

    // The sidecar's enforcement against a peer that really answers: the allow-listed peer is
    // reached, an unlisted host and a redirect hop to one are refused.
    let client_script = dir.path().join("client.py");
    std::fs::write(&client_script, CLIENT).unwrap();
    let client_err = dir.path().join("client.err");
    let client = in_namespace(pid, &python, &client_err)
        .arg(&client_script)
        .arg(ALLOWED_PEER)
        .arg(UNLISTED_HOST)
        .output()
        .unwrap();
    assert!(
        client.status.success(),
        "client failed: {}",
        std::fs::read_to_string(&client_err).unwrap_or_default()
    );
    let outcome: Value = serde_json::from_slice(&client.stdout).unwrap();
    assert_eq!(
        outcome,
        json!({"reached":"peer-ok","unlisted":"refused","redirect":"refused"}),
        "{}",
        std::fs::read_to_string(&client_err).unwrap_or_default()
    );

    let session = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(session.status, 201, "{}", session.body);
    let session = session.body["id"].as_str().unwrap().to_owned();
    let message = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"go","text":"go","mode":"now"})),
        )
        .await;
    assert_eq!(message.status, 202, "{}", message.body);
    let events = settle(&harness, &session, "egress done").await;
    // Observed work must really have happened, or a quiet network proves nothing.
    tool_result(&events, "write");
    assert!(tool_result(&events, "read").to_string().contains("egress"));
    if browser.is_some() {
        assert!(tool_result(&events, "open").to_string().contains("tab_id"));
        assert!(
            tool_result(&events, "repl")
                .to_string()
                .contains("egress fixture"),
            "{}",
            tool_result(&events, "repl")
        );
    }

    let routine = harness
        .request(
            "POST",
            "/v1/routines",
            Some(json!({"name":"egress","schedule":{"type":"interval","secs":3600},
                "action":{"type":"new_session","prompt":"routine run","workspace":harness.workspace()}})),
        )
        .await;
    assert_eq!(routine.status, 201, "{}", routine.body);
    let run = harness
        .request(
            "POST",
            &format!("/v1/routines/{}/run", routine.body["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert!(run.status < 300, "{}", run.body);
    if let Some(routine_session) = run.body["session_id"].as_str() {
        settle(&harness, routine_session, "egress done").await;
    }

    let memory = harness
        .request(
            "POST",
            "/v1/memory",
            Some(json!({"content":"egress fixture prefers loopback"})),
        )
        .await;
    assert!(memory.status < 300, "{}", memory.body);
    let search = harness.request("GET", "/v1/memory?q=loopback", None).await;
    assert_eq!(search.status, 200, "{}", search.body);

    let entries: BTreeSet<String> = std::fs::read_dir(&harness.data_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    harness.kill().await;
    let _ = fixture.kill();
    let _ = fixture.wait();
    let seen = observer.finish();

    let egress = Egress::new(vec![ALLOWED_PEER.to_owned()]).unwrap();
    let blocked = violations(&seen, &egress);
    assert!(
        blocked.is_empty(),
        "non-allow-listed peers contacted: {blocked:?} (all seen: {seen:?})"
    );
    // The capture was live and the allow-listed peer was actually contacted (not merely
    // permitted), while neither refused destination ever saw a packet.
    assert!(
        seen.contains_key(&ALLOWED_PEER.parse().unwrap()),
        "allow-listed peer was never reached: {seen:?}"
    );
    for refused in [UNLISTED_HOST, REDIRECT_HOP] {
        assert!(
            !seen.contains_key(&refused.parse().unwrap()),
            "{refused} was contacted despite not being allow-listed: {seen:?}"
        );
    }
    let peer_requests: Vec<Value> = std::fs::read_to_string(&peer_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let requested: Vec<_> = peer_requests
        .iter()
        .map(|request| {
            (
                request["host"].as_str().unwrap(),
                request["path"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        requested,
        vec![(ALLOWED_PEER, "/"), (ALLOWED_PEER, "/redirect")],
        "the peer saw requests other than the two allowed ones"
    );

    // D-5: every entry the run left in the data directory is documented.
    let architecture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/03-architecture.md"),
    )
    .unwrap();
    let section = architecture
        .split("## 8. 데이터 디렉터리")
        .nth(1)
        .expect("data directory section")
        .split("\n## ")
        .next()
        .unwrap();
    for entry in &entries {
        // SQLite sidecar files belong to their database.
        let base = ["-wal", "-shm", "-journal"]
            .iter()
            .fold(entry.as_str(), |name, suffix| name.trim_end_matches(suffix));
        let documented =
            section.contains(&format!("`{base}`")) || section.contains(&format!("`{base}/`"));
        assert!(
            documented,
            "data directory entry {entry:?} is not documented in docs/03-architecture.md §8"
        );
    }
}

fn host_triple() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("-vV")
        .output()
        .unwrap();
    String::from_utf8(version.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .unwrap()
}

const TELEMETRY_CRATES: &[&str] = &[
    "sentry",
    "sentry-core",
    "opentelemetry-otlp",
    "opentelemetry-http",
    "opentelemetry-jaeger",
    "opentelemetry-zipkin",
    "opentelemetry-datadog",
    "posthog-rs",
    "segment",
    "datadog-apm",
    "rudderanalytics",
    "metrics-exporter-prometheus",
];

const TELEMETRY_PYTHON: &[&str] = &[
    "logfire",
    "sentry-sdk",
    "posthog",
    "opentelemetry-sdk",
    "opentelemetry-exporter-otlp",
    "opentelemetry-exporter-otlp-proto-http",
    "opentelemetry-exporter-otlp-proto-grpc",
    "segment-analytics-python",
    "analytics-python",
    "mixpanel",
    "amplitude-analytics",
    "datadog",
    "ddtrace",
];

/// G-4 / D-5: telemetry is off because no shipped component contains a telemetry sender.
#[test]
fn telemetry_has_no_sender_in_rust_or_python_dependencies() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
        ])
        .arg(host_triple())
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: BTreeSet<&str> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    for name in TELEMETRY_CRATES {
        assert!(
            !names.contains(name),
            "telemetry crate {name} is in the Rust dependency graph"
        );
    }

    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent/uv.lock"))
            .unwrap();
    let packages: BTreeSet<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .collect();
    assert!(!packages.is_empty());
    for name in TELEMETRY_PYTHON {
        assert!(
            !packages.contains(name),
            "telemetry package {name} is in agent/uv.lock"
        );
    }
}
