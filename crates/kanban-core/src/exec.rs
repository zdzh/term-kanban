//! 动作执行：把命令跑起来并记账（见 docs/adr/0006-action-runs-and-log-files.md）。
//!
//! 记账形状与前端无关：一次执行 = 一条 run 记录 + 一份日志文件。
//! TUI 额外把输出 tee 到终端，网页版将来只需要读日志文件。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::model::{Action, ActionKind};
use crate::store::{Result, Store, StoreError};

#[derive(Debug)]
pub enum Outcome {
    /// 链接已经交给系统浏览器。
    LinkOpened,
    /// 命令跑完了。
    Finished {
        run_id: i64,
        exit_code: Option<i32>,
        log_path: PathBuf,
    },
}

/// 数据与日志的落地目录。
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/task-kanban")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("task-kanban")
    }
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// 触发一个动作。
///
/// - 链接：交给系统默认浏览器，不产生 run 记录（没有输出也没有退出码）。
/// - 命令：写一份日志文件、建一条 run 记录，`echo_to_stdout` 决定要不要同时把
///   输出吐到终端（TUI 会先让出界面，再调它）。
pub fn run(store: &Store, action: &Action, echo_to_stdout: bool) -> Result<Outcome> {
    match action.kind {
        ActionKind::Link => {
            open_link(&action.target)?;
            Ok(Outcome::LinkOpened)
        }
        ActionKind::Command => run_command(store, action, echo_to_stdout),
    }
}

fn open_link(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| StoreError::Io(format!("打不开链接 {url}: {e}")))?;
    if !status.success() {
        return Err(StoreError::Io(format!("打开链接失败: {url}")));
    }
    Ok(())
}

fn run_command(store: &Store, action: &Action, echo_to_stdout: bool) -> Result<Outcome> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(format!("建日志目录失败: {e}")))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let log_path = dir.join(format!("action-{}-{}.log", action.id, stamp));
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| StoreError::Io(format!("建日志文件失败: {e}")))?;

    let run_id = store.start_run(action.id, &log_path.to_string_lossy())?;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&action.target)
        .stdin(if action.needs_terminal {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| StoreError::Io(format!("启动命令失败: {e}")))?;

    let log = Arc::new(Mutex::new(log_file));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_tee(out, log.clone(), echo_to_stdout));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_tee(err, log.clone(), echo_to_stdout));
    }
    for r in readers {
        let _ = r.join();
    }

    let status = child
        .wait()
        .map_err(|e| StoreError::Io(format!("等待命令结束失败: {e}")))?;
    let code = status.code();
    store.finish_run(run_id, code)?;

    {
        let mut f = log.lock().unwrap();
        let _ = writeln!(
            f,
            "\n--- 退出码 {} · {} ---",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "被信号终止".into()),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }

    Ok(Outcome::Finished {
        run_id,
        exit_code: code,
        log_path,
    })
}

fn spawn_tee<R: Read + Send + 'static>(
    mut src: R,
    log: Arc<Mutex<File>>,
    echo: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    {
                        let mut f = log.lock().unwrap();
                        let _ = f.write_all(&buf[..n]);
                        let _ = f.flush();
                    }
                    if echo {
                        let mut out = std::io::stdout();
                        let _ = out.write_all(&buf[..n]);
                        let _ = out.flush();
                    }
                }
                Err(_) => break,
            }
        }
    })
}
