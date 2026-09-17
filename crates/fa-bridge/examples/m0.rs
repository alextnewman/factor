// M0 spike: Rust drives the pwsh JSON-RPC bridge over stdio.
// (Windows will use a named pipe with identical framing.)
//
// Measures:
//   1. cold start: spawn -> first response
//   2. correctness: Read-FAFile round trip
//   3. error propagation: missing file -> JSON-RPC error object
//   4. round-trip latency distribution over 1000 calls
//   5. kill reliability: SIGKILL a hung pwsh, time to reaping
//
// Run: cargo run --example m0 [-- /path/to/bridge.ps1]

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

struct Bridge {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Bridge {
    fn spawn(bridge_ps1: &str) -> std::io::Result<(Self, Duration)> {
        let t0 = Instant::now();
        let mut child = Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File", bridge_ps1])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = BufWriter::new(child.stdin.take().expect("bridge stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("bridge stdout"));
        let mut b = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        // Handshake doubles as the cold-start measurement.
        let _ = b.call("Read-FAFile", r#"{"Path":"/tmp/m0-test.txt"}"#)?;
        Ok((b, t0.elapsed()))
    }

    fn call(
        &mut self,
        method: &str,
        params_json: &str,
    ) -> std::io::Result<(serde_json::Value, Duration)> {
        let id = self.next_id;
        self.next_id += 1;
        let req = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params_json}}}"
        );
        let t = Instant::now();
        self.stdin.write_all(req.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        let elapsed = t.elapsed();
        let v: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok((v, elapsed))
    }

    fn shutdown(mut self) {
        drop(self.stdin); // EOF on stdin: the bridge exits itself
        let _ = self.child.wait();
    }
}

fn main() -> std::io::Result<()> {
    let bridge_ps1 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "psmodule/FactorAgent/bridge.ps1".to_string());
    std::fs::write("/tmp/m0-test.txt", "hello from the spike\nsecond line\n")?;

    let (mut b, cold_start) = Bridge::spawn(&bridge_ps1)?;
    println!("cold start (spawn -> first response): {cold_start:?}");

    // Correctness.
    let (v, _) = b.call("Read-FAFile", r#"{"Path":"/tmp/m0-test.txt"}"#)?;
    assert_eq!(v["result"], "hello from the spike\nsecond line\n");
    assert_eq!(v["id"], 2);
    println!("correctness: ok");

    // Error propagation.
    let (v, _) = b.call("Read-FAFile", r#"{"Path":"/tmp/does-not-exist.txt"}"#)?;
    assert!(v.get("error").is_some(), "expected JSON-RPC error object");
    println!("error propagation: ok [{}]", v["error"]["message"]);

    // Latency distribution.
    let n = 1000;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let (_, d) = b.call("Read-FAFile", r#"{"Path":"/tmp/m0-test.txt"}"#)?;
        samples.push(d.as_micros() as u64);
    }
    samples.sort_unstable();
    let mean = samples.iter().sum::<u64>() as f64 / n as f64;
    println!(
        "round-trip x{n} (µs): mean={mean:.0} p50={} p99={} max={}",
        samples[n / 2],
        samples[n * 99 / 100],
        samples[n - 1]
    );

    b.shutdown();
    println!("clean shutdown on stdin EOF: ok");

    // Kill reliability: a hung pwsh must die promptly on kill().
    let mut sleeper = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep 300"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::sleep(Duration::from_millis(500));
    let t = Instant::now();
    sleeper.kill()?;
    let status = sleeper.wait()?;
    println!("kill: reaped {:?} after signal (status: {status})", t.elapsed());
    assert!(!status.success());

    println!("M0: all green");
    Ok(())
}
