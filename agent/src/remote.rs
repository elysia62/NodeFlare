use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteTaskMessage {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) task_id: String,
    pub(crate) command: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TaskResultMessage {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) task_id: String,
    pub(crate) status: String,
    pub(crate) result: String,
    pub(crate) exit_code: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskResultAckMessage {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) task_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteTaskJournal {
    pub(crate) version: u32,
    pub(crate) entries: Vec<RemoteTaskJournalEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum RemoteTaskJournalEntry {
    Active { task_id: String },
    Completed { result: TaskResultMessage },
}

#[derive(Default)]
pub(crate) struct CapturedOutput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

pub(crate) fn capture_remote_output<R>(mut reader: R) -> mpsc::Receiver<CapturedOutput>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        // Keep draining after the limit so verbose commands do not receive SIGPIPE.
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let retained = read.min(REMOTE_STREAM_OUTPUT_BYTES as usize - bytes.len());
                    bytes.extend_from_slice(&buffer[..retained]);
                    truncated |= retained < read;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = sender.send(CapturedOutput { bytes, truncated });
    });
    receiver
}

pub(crate) fn terminate_remote_process_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .args(["-KILL", "--", process_group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn receive_remote_output(
    receiver: &mpsc::Receiver<CapturedOutput>,
    child: &mut Child,
) -> CapturedOutput {
    match receiver.recv_timeout(REMOTE_OUTPUT_DRAIN_TIMEOUT) {
        Ok(output) => output,
        Err(_) => {
            // Descendants may keep the pipe open after the shell exits.
            terminate_remote_process_tree(child);
            receiver
                .recv_timeout(REMOTE_OUTPUT_DRAIN_TIMEOUT)
                .unwrap_or_default()
        }
    }
}

pub(crate) fn remote_result_text(stdout: &CapturedOutput, stderr: &CapturedOutput) -> String {
    let truncated = stdout.truncated || stderr.truncated;
    let stdout = String::from_utf8_lossy(&stdout.bytes);
    let stderr = String::from_utf8_lossy(&stderr.bytes);
    let mut combined = if stderr.is_empty() {
        stdout.trim().to_string()
    } else if stdout.is_empty() {
        stderr.trim().to_string()
    } else {
        format!("{}\n{}", stdout.trim(), stderr.trim())
    };
    let result_truncated = combined.len() > REMOTE_RESULT_OUTPUT_BYTES;
    if result_truncated {
        let mut end = REMOTE_RESULT_OUTPUT_BYTES;
        while !combined.is_char_boundary(end) {
            end -= 1;
        }
        combined.truncate(end);
    }
    if truncated || result_truncated {
        combined.push_str("\n[输出已截断]");
    }
    combined
}

pub(crate) fn wait_for_remote_child(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

pub(crate) fn execute_remote_task_with_timeout(
    task: &RemoteTaskMessage,
    timeout: Duration,
) -> TaskResultMessage {
    let task_id = task.task_id.clone();

    if task.command.trim().is_empty() || task.command.len() > 16_384 {
        return TaskResultMessage {
            message_type: "task_result".to_string(),
            task_id,
            status: "failed".to_string(),
            result: "命令为空或长度超出限制".to_string(),
            exit_code: Some(-1),
        };
    }

    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command.args(["-c", task.command.as_str()]);
        command.process_group(0);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", task.command.as_str()]);
        command
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TaskResultMessage {
                message_type: "task_result".to_string(),
                task_id,
                status: "failed".to_string(),
                result: format!("无法启动命令：{error}"),
                exit_code: Some(-1),
            };
        }
    };
    let stdout = capture_remote_output(child.stdout.take().expect("stdout is piped"));
    let stderr = capture_remote_output(child.stderr.take().expect("stderr is piped"));

    let status = match wait_for_remote_child(&mut child, timeout) {
        Ok(Some(status)) => Some(status),
        Ok(None) => {
            terminate_remote_process_tree(&mut child);
            None
        }
        Err(error) => {
            terminate_remote_process_tree(&mut child);
            let stdout = receive_remote_output(&stdout, &mut child);
            let stderr = receive_remote_output(&stderr, &mut child);
            let output = remote_result_text(&stdout, &stderr);
            return TaskResultMessage {
                message_type: "task_result".to_string(),
                task_id,
                status: "failed".to_string(),
                result: if output.is_empty() {
                    format!("等待命令结束失败：{error}")
                } else {
                    format!("{output}\n等待命令结束失败：{error}")
                },
                exit_code: Some(-1),
            };
        }
    };
    let stdout = receive_remote_output(&stdout, &mut child);
    let stderr = receive_remote_output(&stderr, &mut child);
    let output = remote_result_text(&stdout, &stderr);

    let Some(status) = status else {
        let timeout_seconds = timeout.as_secs().max(1);
        return TaskResultMessage {
            message_type: "task_result".to_string(),
            task_id,
            status: "failed".to_string(),
            result: if output.is_empty() {
                format!("命令执行超过 {timeout_seconds} 秒，已终止")
            } else {
                format!("{output}\n命令执行超过 {timeout_seconds} 秒，已终止")
            },
            exit_code: Some(-1),
        };
    };

    TaskResultMessage {
        message_type: "task_result".to_string(),
        task_id,
        status: if status.success() {
            "success"
        } else {
            "failed"
        }
        .to_string(),
        result: output,
        exit_code: status.code().map(i64::from),
    }
}

pub(crate) fn execute_remote_task(task: &RemoteTaskMessage) -> TaskResultMessage {
    execute_remote_task_with_timeout(task, REMOTE_TASK_TIMEOUT)
}

pub(crate) struct RemoteExecutor {
    pub(crate) task_tx: mpsc::SyncSender<RemoteTaskMessage>,
    pub(crate) result_rx: mpsc::Receiver<TaskResultMessage>,
    pub(crate) active: HashSet<String>,
    pub(crate) completed: HashMap<String, TaskResultMessage>,
    pub(crate) task_order: VecDeque<String>,
    pub(crate) last_sent: HashMap<String, Instant>,
    pub(crate) journal_path: PathBuf,
    pub(crate) disabled: bool,
}

impl RemoteExecutor {
    pub(crate) fn new() -> Result<Self> {
        Self::new_at(remote_task_journal_path()?)
    }

    pub(crate) fn new_at(journal_path: PathBuf) -> Result<Self> {
        let journal = load_remote_task_journal(&journal_path)?;
        let mut active = HashSet::new();
        let mut completed = HashMap::new();
        let mut task_order = VecDeque::new();
        for entry in journal.entries {
            match entry {
                RemoteTaskJournalEntry::Active { task_id } => {
                    active.insert(task_id.clone());
                    task_order.push_back(task_id);
                }
                RemoteTaskJournalEntry::Completed { result } => {
                    task_order.push_back(result.task_id.clone());
                    completed.insert(result.task_id.clone(), result);
                }
            }
        }

        let (task_tx, task_rx) = mpsc::sync_channel::<RemoteTaskMessage>(16);
        let (result_tx, result_rx) = mpsc::channel::<TaskResultMessage>();
        thread::Builder::new()
            .name("nodeflare-remote".to_string())
            .spawn(move || {
                while let Ok(task) = task_rx.recv() {
                    if result_tx.send(execute_remote_task(&task)).is_err() {
                        return;
                    }
                }
            })?;
        let interrupted = active.drain().collect::<Vec<_>>();
        for task_id in interrupted {
            completed.insert(
                task_id.clone(),
                TaskResultMessage {
                    message_type: "task_result".to_string(),
                    task_id,
                    status: "failed".to_string(),
                    result: "Agent 在命令执行期间重启，无法确认原命令状态；为避免重复操作，任务不会再次执行"
                        .to_string(),
                    exit_code: Some(-1),
                },
            );
        }
        let executor = Self {
            task_tx,
            result_rx,
            active,
            completed,
            task_order,
            last_sent: HashMap::new(),
            journal_path,
            disabled: false,
        };
        if !executor.completed.is_empty() {
            executor.persist_snapshot(None)?;
        }
        Ok(executor)
    }

    pub(crate) fn disabled() -> Self {
        let (task_tx, task_rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::channel();
        drop(task_rx);
        drop(result_tx);
        Self {
            task_tx,
            result_rx,
            active: HashSet::new(),
            completed: HashMap::new(),
            task_order: VecDeque::new(),
            last_sent: HashMap::new(),
            journal_path: PathBuf::new(),
            disabled: true,
        }
    }

    pub(crate) fn enqueue(&mut self, task: RemoteTaskMessage) -> Option<TaskResultMessage> {
        if self.disabled {
            return Some(remote_task_rejected(
                task.task_id,
                "Agent 任务日志异常，远程执行已禁用",
            ));
        }
        if self.completed.contains_key(&task.task_id) {
            self.last_sent.remove(&task.task_id);
            return None;
        }
        if self.active.contains(&task.task_id) {
            return None;
        }
        let task_id = task.task_id.clone();
        if self.task_order.len() >= MAX_REMOTE_TASK_JOURNAL_ENTRIES {
            return Some(remote_task_rejected(
                task_id,
                "待确认的远程任务过多，已拒绝执行",
            ));
        }
        self.active.insert(task_id.clone());
        self.task_order.push_back(task_id.clone());
        if let Err(error) = self.persist_snapshot(None) {
            self.active.remove(&task_id);
            self.task_order.retain(|id| id != &task_id);
            eprintln!("remote task journal write failed: {error}");
            return Some(remote_task_rejected(
                task_id,
                "Agent 无法持久化任务状态，已拒绝执行",
            ));
        }
        match self.task_tx.try_send(task) {
            Ok(()) => None,
            Err(error) => {
                let message = match error {
                    mpsc::TrySendError::Full(_) => "远程命令队列已满，请稍后重试",
                    mpsc::TrySendError::Disconnected(_) => "远程命令执行线程不可用",
                };
                let result = remote_task_rejected(task_id, message);
                self.remember_completed(result.clone());
                None
            }
        }
    }

    pub(crate) fn drain_completed(&mut self) {
        let results = self.result_rx.try_iter().collect::<Vec<_>>();
        for result in results {
            self.remember_completed(result);
        }
    }

    pub(crate) fn acknowledge(&mut self, task_id: &str) {
        if !self.completed.contains_key(task_id) {
            return;
        }
        match self.persist_snapshot(Some(task_id)) {
            Ok(()) => {
                self.completed.remove(task_id);
                self.task_order.retain(|id| id != task_id);
                self.last_sent.remove(task_id);
            }
            Err(error) => {
                eprintln!("remote task ACK persistence failed: {error}");
            }
        }
    }

    pub(crate) fn remember_completed(&mut self, result: TaskResultMessage) {
        let task_id = result.task_id.clone();
        let was_active = self.active.remove(&task_id);
        if !was_active && !self.completed.contains_key(&task_id) {
            self.task_order.push_back(task_id.clone());
        }
        self.completed.insert(task_id.clone(), result);
        self.last_sent.remove(&task_id);
        if let Err(error) = self.persist_snapshot(None) {
            eprintln!("remote task result persistence failed: {error}");
        }
    }

    pub(crate) fn due_results(&mut self) -> Vec<TaskResultMessage> {
        self.drain_completed();
        let now = Instant::now();
        self.task_order
            .iter()
            .filter_map(|task_id| {
                let result = self.completed.get(task_id)?;
                let due = self.last_sent.get(task_id).is_none_or(|sent_at| {
                    now.duration_since(*sent_at) >= REMOTE_RESULT_RETRY_INTERVAL
                });
                due.then(|| result.clone())
            })
            .collect()
    }

    pub(crate) fn mark_sent(&mut self, task_id: &str) {
        self.last_sent.insert(task_id.to_string(), Instant::now());
    }

    pub(crate) fn reset_delivery(&mut self) {
        self.last_sent.clear();
    }

    pub(crate) fn persist_snapshot(&self, excluded_task_id: Option<&str>) -> Result<()> {
        let entries = self
            .task_order
            .iter()
            .filter(|task_id| excluded_task_id != Some(task_id.as_str()))
            .filter_map(|task_id| {
                if self.active.contains(task_id) {
                    Some(RemoteTaskJournalEntry::Active {
                        task_id: task_id.clone(),
                    })
                } else {
                    self.completed
                        .get(task_id)
                        .cloned()
                        .map(|result| RemoteTaskJournalEntry::Completed { result })
                }
            })
            .collect::<Vec<_>>();
        write_remote_task_journal(&self.journal_path, entries)
    }
}

pub(crate) fn remote_task_rejected(task_id: String, result: &str) -> TaskResultMessage {
    TaskResultMessage {
        message_type: "task_result".to_string(),
        task_id,
        status: "failed".to_string(),
        result: result.to_string(),
        exit_code: Some(-1),
    }
}

pub(crate) fn valid_remote_task_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn load_remote_task_journal(path: &Path) -> Result<RemoteTaskJournal> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(RemoteTaskJournal {
                version: REMOTE_TASK_JOURNAL_VERSION,
                entries: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    if bytes.len() as u64 > MAX_REMOTE_TASK_JOURNAL_BYTES {
        return Err("remote task journal is too large".into());
    }
    let journal = serde_json::from_slice::<RemoteTaskJournal>(&bytes)?;
    if journal.version != REMOTE_TASK_JOURNAL_VERSION {
        return Err("unsupported remote task journal version".into());
    }
    if journal.entries.len() > MAX_REMOTE_TASK_JOURNAL_ENTRIES {
        return Err("remote task journal has too many entries".into());
    }
    let mut task_ids = HashSet::new();
    for entry in &journal.entries {
        let (task_id, valid_result) = match entry {
            RemoteTaskJournalEntry::Active { task_id } => (task_id, true),
            RemoteTaskJournalEntry::Completed { result } => (
                &result.task_id,
                result.message_type == "task_result"
                    && matches!(result.status.as_str(), "success" | "failed")
                    && result.result.len() <= REMOTE_RESULT_OUTPUT_BYTES + 64,
            ),
        };
        if !valid_remote_task_id(task_id) || !valid_result || !task_ids.insert(task_id) {
            return Err("remote task journal contains an invalid entry".into());
        }
    }
    Ok(journal)
}

pub(crate) fn write_remote_task_journal(
    path: &Path,
    entries: Vec<RemoteTaskJournalEntry>,
) -> Result<()> {
    if entries.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let journal = RemoteTaskJournal {
        version: REMOTE_TASK_JOURNAL_VERSION,
        entries,
    };
    let mut encoded = serde_json::to_vec(&journal)?;
    encoded.push(b'\n');
    if encoded.len() as u64 > MAX_REMOTE_TASK_JOURNAL_BYTES {
        return Err("remote task journal is too large".into());
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("remote-tasks"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    drop(file);
    // fs::rename maps to MoveFileEx(MOVEFILE_REPLACE_EXISTING) on Windows, so it
    // replaces the destination atomically; deleting it first would open a window
    // where a crash loses the whole journal.
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}
