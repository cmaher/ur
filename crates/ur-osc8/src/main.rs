//! `ur-osc8` — transparent PTY filter that injects OSC 8 hyperlinks.
//!
//! Usage:
//!
//! ```text
//! ur-osc8 -- <command> [args...]
//! ```
//!
//! Spawns `<command>` on a pseudo-terminal, pumps the child's output through
//! [`ur_osc8::Injector`] to wrap bare URLs in OSC 8 hyperlink escapes, and
//! forwards stdin plus `SIGWINCH` to the child. Exits with the child's status.
//!
//! Designed to run as a wrapper inside worker containers, e.g.
//! `ur-osc8 -- claude`, so downstream tools (tmux, the host terminal) see
//! pre-hyperlinked output.
//!
//! No stdout buffering is introduced beyond the injector's own short
//! `pending` window — output is flushed to stdout after every chunk.

use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use ur_osc8::Injector;

/// Parsed CLI: everything after the `--` separator is the child command.
struct Cli {
    program: String,
    args: Vec<String>,
}

fn parse_args() -> Result<Cli> {
    let mut argv = std::env::args().skip(1);
    let mut rest: Vec<String> = Vec::new();
    let mut saw_sep = false;
    for a in argv.by_ref() {
        if !saw_sep && a == "--" {
            saw_sep = true;
            continue;
        }
        if !saw_sep {
            // Allow omitting `--` for convenience, but the ticket spec uses it.
            // Treat any pre-`--` token as the start of the command if no `--`
            // has been seen yet.
            rest.push(a);
            saw_sep = true;
            continue;
        }
        rest.push(a);
    }
    if rest.is_empty() {
        return Err(anyhow!("usage: ur-osc8 -- <command> [args...]"));
    }
    let program = rest.remove(0);
    Ok(Cli {
        program,
        args: rest,
    })
}

/// Best-effort query of the controlling terminal's current size. Falls back to
/// an 80x24 default on failure (e.g. when stdout is not a tty).
fn current_pty_size() -> PtySize {
    let mut size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    // SAFETY: TIOCGWINSZ is a read-only ioctl that fills a `winsize` struct.
    // We only inspect the result on success.
    #[cfg(unix)]
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_row > 0
            && ws.ws_col > 0
        {
            size.rows = ws.ws_row;
            size.cols = ws.ws_col;
            size.pixel_width = ws.ws_xpixel;
            size.pixel_height = ws.ws_ypixel;
        }
    }
    size
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ur-osc8: {e}");
            return ExitCode::from(2);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ur-osc8: failed to start runtime: {e}");
            return ExitCode::from(1);
        }
    };

    match rt.block_on(run(cli)) {
        Ok(code) => {
            // Preserve non-u8 exit codes as best we can — ExitCode is u8 on
            // Unix, but callers that care about signals/large codes will see
            // the low 8 bits, matching shell behaviour.
            ExitCode::from((code & 0xff) as u8)
        }
        Err(e) => {
            eprintln!("ur-osc8: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<i32> {
    let pty_system = native_pty_system();
    let initial_size = current_pty_size();
    let pair = pty_system
        .openpty(initial_size)
        .context("failed to open pty")?;

    let mut cmd = CommandBuilder::new(&cli.program);
    for a in &cli.args {
        cmd.arg(a);
    }
    // Inherit the parent's environment and working directory.
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("failed to spawn {}", cli.program))?;

    // The slave handle is no longer needed in the parent process; dropping it
    // avoids keeping an extra reference open which would prevent EOF on the
    // master reader when the child exits.
    drop(pair.slave);

    let master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>> =
        Arc::new(Mutex::new(pair.master));

    // --- Output pump: master → injector → stdout ------------------------
    let master_reader = master
        .lock()
        .map_err(|_| anyhow!("master mutex poisoned"))?
        .try_clone_reader()
        .context("failed to clone pty reader")?;
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
    let reader_handle = std::thread::spawn(move || pty_reader_loop(master_reader, out_tx));
    let output_task = tokio::spawn(output_pump(out_rx));

    // --- Input pump: stdin → master writer ------------------------------
    let master_writer = master
        .lock()
        .map_err(|_| anyhow!("master mutex poisoned"))?
        .take_writer()
        .context("failed to take pty writer")?;
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
    let writer_handle = std::thread::spawn(move || pty_writer_loop(master_writer, in_rx));
    let input_task = tokio::spawn(stdin_pump(in_tx));

    // --- SIGWINCH forwarding --------------------------------------------
    #[cfg(unix)]
    let winch_task = {
        let master = Arc::clone(&master);
        tokio::spawn(sigwinch_loop(master))
    };

    // --- Wait for the child --------------------------------------------
    // `child.wait()` is blocking; run it on a blocking thread so the async
    // pumps keep making progress.
    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .context("child wait task panicked")?
        .context("failed to wait for child")?;

    // Stop the input pump — the child is gone.
    #[cfg(unix)]
    winch_task.abort();
    input_task.abort();

    // Let the reader drain any remaining output (EOF will arrive once the
    // master's last writer reference is dropped).
    let _ = reader_handle.join();
    // out_tx was moved into the reader thread; it is dropped on exit, which
    // closes out_rx and lets the output task finish.
    if let Err(e) = output_task.await {
        eprintln!("ur-osc8: output task error: {e}");
    }

    // Drop the writer-side channel by aborting the writer thread's source;
    // in_tx was moved into `input_task`, which is now aborted. The writer
    // thread will see `recv() == None` on its next iteration and exit.
    drop(writer_handle); // detach; thread will exit on channel close

    let code = status.exit_code() as i32;
    Ok(code)
}

/// Blocking loop that reads bytes from the pty master and forwards them over
/// a channel to the async output pump.
fn pty_reader_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Blocking loop that writes bytes received over a channel to the pty master.
fn pty_writer_loop(mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(chunk) = rx.blocking_recv() {
        if writer.write_all(&chunk).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}

/// Async task: consume PTY output chunks, run them through the OSC 8
/// injector, and write the transformed bytes to stdout. If the injector
/// panics, the panic is caught and surfaced to stderr so the child does not
/// die silently.
async fn output_pump(mut rx: mpsc::Receiver<Vec<u8>>) {
    let mut injector = Injector::new();
    let mut stdout = tokio::io::stdout();
    let mut out_buf: Vec<u8> = Vec::with_capacity(8192);

    while let Some(chunk) = rx.recv().await {
        out_buf.clear();
        let inject_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            injector.push(&chunk, &mut out_buf);
        }));
        match inject_result {
            Ok(()) => {
                if let Err(e) = stdout.write_all(&out_buf).await {
                    eprintln!("ur-osc8: stdout write failed: {e}");
                    return;
                }
                if let Err(e) = stdout.flush().await {
                    eprintln!("ur-osc8: stdout flush failed: {e}");
                    return;
                }
            }
            Err(_) => {
                eprintln!("ur-osc8: injector panicked; passing bytes through verbatim");
                // Best-effort: emit the raw chunk so the user still sees output.
                let _ = stdout.write_all(&chunk).await;
                let _ = stdout.flush().await;
                // Reset the injector to a known-good state.
                injector = Injector::new();
            }
        }
    }

    // End of stream: flush any held bytes.
    out_buf.clear();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        injector.flush(&mut out_buf);
    }));
    let _ = stdout.write_all(&out_buf).await;
    let _ = stdout.flush().await;
}

/// Async task: read bytes from this process's stdin and forward them to the
/// pty master's writer via a blocking channel.
async fn stdin_pump(tx: mpsc::Sender<Vec<u8>>) {
    let mut stdin = tokio::io::stdin();
    let mut buf = vec![0u8; 4096];
    loop {
        match stdin.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
async fn sigwinch_loop(master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sig = match signal(SignalKind::window_change()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ur-osc8: SIGWINCH handler setup failed: {e}");
            return;
        }
    };
    while sig.recv().await.is_some() {
        let size = current_pty_size();
        if let Ok(guard) = master.lock()
            && let Err(e) = guard.resize(size)
        {
            eprintln!("ur-osc8: pty resize failed: {e}");
        }
    }
}
