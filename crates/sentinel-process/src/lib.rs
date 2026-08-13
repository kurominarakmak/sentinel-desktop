#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
    time,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    Stdout(String),
    Stderr(String),
    OutputError,
    Exited(Option<i32>),
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("process has no id")]
    MissingId,
    #[error("owned process group did not terminate")]
    GroupStillAlive,
}

pub struct SupervisedProcess {
    child: Child,
    pid: u32,
}

impl SupervisedProcess {
    pub async fn start(
        program: &str,
        args: &[&str],
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), ProcessError> {
        Self::start_in(program, args, None).await
    }

    pub async fn start_in(
        program: &str,
        args: &[&str],
        working_directory: Option<&Path>,
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), ProcessError> {
        Self::start_in_with_environment(program, args, working_directory, false).await
    }

    /// Starts a provider with a fixed empty environment.  Generic repository
    /// helpers intentionally retain their existing environment semantics, so
    /// execution adapters must opt into this narrower boundary explicitly.
    pub async fn start_in_sanitized(
        program: &str,
        args: &[&str],
        working_directory: Option<&Path>,
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), ProcessError> {
        Self::start_in_with_environment(program, args, working_directory, true).await
    }

    pub async fn start_in_sanitized_with_env(
        program: &str,
        args: &[&str],
        working_directory: Option<&Path>,
        environment: &[(String, String)],
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), ProcessError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .env_clear()
            .envs(environment.iter().cloned());
        if let Some(directory) = working_directory {
            command.current_dir(directory);
        }
        #[cfg(unix)]
        unsafe {
            command.as_std_mut().pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let mut child = command.spawn()?;
        let pid = child.id().ok_or(ProcessError::MissingId)?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (sender, receiver) = mpsc::channel(64);
        if let Some(stdout) = stdout {
            forward_lines(BufReader::new(stdout), sender.clone(), ProcessEvent::Stdout);
        }
        if let Some(stderr) = stderr {
            forward_lines(BufReader::new(stderr), sender.clone(), ProcessEvent::Stderr);
        }
        tokio::spawn(async move {
            let _ = sender;
        });
        Ok((Self { child, pid }, receiver))
    }

    async fn start_in_with_environment(
        program: &str,
        args: &[&str],
        working_directory: Option<&Path>,
        sanitize_environment: bool,
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), ProcessError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if sanitize_environment {
            command.stdin(Stdio::null()).env_clear();
        }
        if let Some(working_directory) = working_directory {
            command.current_dir(working_directory);
        }
        #[cfg(unix)]
        unsafe {
            command.as_std_mut().pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let mut child = command.spawn()?;
        let pid = child.id().ok_or(ProcessError::MissingId)?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (sender, receiver) = mpsc::channel(64);
        if let Some(stdout) = stdout {
            forward_lines(BufReader::new(stdout), sender.clone(), ProcessEvent::Stdout);
        }
        if let Some(stderr) = stderr {
            forward_lines(BufReader::new(stderr), sender.clone(), ProcessEvent::Stderr);
        }
        tokio::spawn(async move {
            let _ = sender;
        });
        Ok((Self { child, pid }, receiver))
    }

    pub async fn wait(&mut self) -> Result<Option<i32>, ProcessError> {
        Ok(self.child.wait().await?.code())
    }

    pub fn try_wait(&mut self) -> Result<Option<Option<i32>>, ProcessError> {
        Ok(self.child.try_wait()?.map(|status| status.code()))
    }

    pub async fn cancel(&mut self, timeout: Duration) -> Result<Option<i32>, ProcessError> {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }
        let code: Result<Option<i32>, ProcessError> =
            match time::timeout(timeout, self.child.wait()).await {
                Ok(result) => Ok(result?.code()),
                Err(_) => {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(self.pid as i32), libc::SIGKILL);
                    }
                    #[cfg(not(unix))]
                    {
                        self.child.kill().await?;
                    }
                    Ok(self.child.wait().await?.code())
                }
            };
        let code = code?;
        #[cfg(unix)]
        {
            if self.process_group_is_alive() && !wait_for_group_exit(self.pid, timeout).await {
                unsafe {
                    libc::kill(-(self.pid as i32), libc::SIGKILL);
                }
                if !wait_for_group_exit(self.pid, timeout).await {
                    return Err(ProcessError::GroupStillAlive);
                }
            }
        }
        Ok(code)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    #[cfg(unix)]
    pub fn process_group_is_alive(&self) -> bool {
        unsafe { libc::kill(-(self.pid as i32), 0) == 0 }
    }
}

#[cfg(unix)]
async fn wait_for_group_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = time::Instant::now() + timeout;
    while time::Instant::now() < deadline {
        if unsafe { libc::kill(-(pid as i32), 0) != 0 } {
            return true;
        }
        time::sleep(Duration::from_millis(10)).await;
    }
    unsafe { libc::kill(-(pid as i32), 0) != 0 }
}

fn forward_lines<R>(
    reader: BufReader<R>,
    sender: mpsc::Sender<ProcessEvent>,
    event: fn(String) -> ProcessEvent,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if sender.send(event(line)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    let _ = sender.send(ProcessEvent::OutputError).await;
                    break;
                }
            }
        }
    });
}
