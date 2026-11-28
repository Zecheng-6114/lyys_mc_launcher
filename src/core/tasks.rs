//! 实例安装任务管理器：维护任务队列、分配并执行安装任务。
//!
//! 任务在独立线程上运行，进度与结果通过构造时传入的回调上报，
//! 调用方（图形界面或命令行）不需要了解安装细节。

use std::path::PathBuf;
use std::sync::Arc;

use mc_launcher_core::loader::common::LoaderSpec;

use super::i18n::tf;
use super::install::{self, InstallProgress};
use super::logging;

/// 单个安装任务的状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallTaskStatus {
    Queued,
    Running,
    Done,
    Failed,
}

/// 一次安装任务的提交参数，目录、Java 与镜像在提交时快照。
#[derive(Clone)]
pub struct InstallRequest {
    pub version: String,
    /// 用户为实例起的显示名，空串表示不命名。
    pub name: String,
    pub loader: Option<LoaderSpec>,
    /// 加载器显示名，用于任务标题与结果文案。
    pub loader_label: &'static str,
    /// 用户输入的加载器版本，空串表示最新。
    pub loader_version: String,
    pub dir: PathBuf,
    pub java: PathBuf,
    pub mirror: String,
}

/// 任务管理器记录的一次安装任务。
pub struct InstallTask {
    pub id: u64,
    pub request: InstallRequest,
    pub status: InstallTaskStatus,
    pub progress: Option<InstallProgress>,
    pub message: Option<String>,
}

/// 任务管理器向调用方上报的事件。
pub enum TaskEvent {
    Progress { id: u64, progress: InstallProgress },
    Done {
        id: u64,
        ok: bool,
        message: String,
        /// 安装成功时实际写入的版本 id，调用方据此记录实例显示名。
        version_id: Option<String>,
    },
}

pub struct TaskManager {
    tasks: Vec<InstallTask>,
    next_id: u64,
    on_event: Arc<dyn Fn(TaskEvent) + Send + Sync>,
}

impl TaskManager {
    pub fn new(on_event: impl Fn(TaskEvent) + Send + Sync + 'static) -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
            on_event: Arc::new(on_event),
        }
    }

    pub fn tasks(&self) -> &[InstallTask] {
        &self.tasks
    }

    /// 把任务加入队列；同一目录下已有相同参数的未完成任务时返回 false。
    pub fn submit(&mut self, request: InstallRequest) -> bool {
        let duplicate = self.tasks.iter().any(|task| {
            matches!(
                task.status,
                InstallTaskStatus::Queued | InstallTaskStatus::Running
            ) && task.request.dir == request.dir
                && task.request.version == request.version
                && task.request.loader == request.loader
                && task.request.loader_version == request.loader_version
        });
        if duplicate {
            return false;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(InstallTask {
            id,
            request,
            status: InstallTaskStatus::Queued,
            progress: None,
            message: None,
        });
        true
    }

    /// 启动所有排队中的任务，任务数量不设上限。
    pub fn dispatch(&mut self) {
        for task in self
            .tasks
            .iter_mut()
            .filter(|task| task.status == InstallTaskStatus::Queued)
        {
            task.status = InstallTaskStatus::Running;
            let id = task.id;
            let request = task.request.clone();
            let on_event = Arc::clone(&self.on_event);
            std::thread::spawn(move || {
                let mut reporter = |progress: InstallProgress| {
                    on_event(TaskEvent::Progress { id, progress });
                };
                let (ok, message, version_id) = run_task(&request, &mut reporter);
                on_event(TaskEvent::Done {
                    id,
                    ok,
                    message,
                    version_id,
                });
            });
        }
    }

    pub fn update_progress(&mut self, id: u64, progress: InstallProgress) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
            task.progress = Some(progress);
        }
    }

    pub fn finish(&mut self, id: u64, ok: bool, message: String) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
            task.status = if ok {
                InstallTaskStatus::Done
            } else {
                InstallTaskStatus::Failed
            };
            task.progress = None;
            task.message = Some(message);
        }
    }

    /// 移除已完成或失败的任务，排队与运行中的任务不受影响。
    pub fn remove(&mut self, id: u64) {
        self.tasks.retain(|task| {
            task.id != id
                || matches!(
                    task.status,
                    InstallTaskStatus::Queued | InstallTaskStatus::Running
                )
        });
    }
}

fn run_task(
    request: &InstallRequest,
    reporter: &mut dyn FnMut(InstallProgress),
) -> (bool, String, Option<String>) {
    match &request.loader {
        Some(loader) => {
            match install::install_loader_with_reporter(
                &request.dir,
                &request.version,
                loader.clone(),
                &request.java,
                &request.mirror,
                reporter,
            ) {
                Ok(result) => {
                    logging::info(&format!(
                        "installed {} {} into {}",
                        request.loader_label,
                        result.version_id,
                        request.dir.display()
                    ));
                    let message = tf(
                        "install.installed",
                        &[
                            ("version", request.loader_label),
                            ("loader", result.version_id.as_str()),
                        ],
                    );
                    (true, message, Some(result.version_id))
                }
                Err(error) => {
                    let error = error.to_string();
                    logging::error("install", &error);
                    (
                        false,
                        tf("error.install_failed", &[("error", error.as_str())]),
                        None,
                    )
                }
            }
        }
        None => {
            match install::install_with_reporter(
                &request.dir,
                &request.version,
                &request.mirror,
                reporter,
            ) {
                Ok(result) => {
                    logging::info(&format!(
                        "installed {} into {}",
                        request.version,
                        request.dir.display()
                    ));
                    (
                        true,
                        tf("install.installed_only", &[("version", request.version.as_str())]),
                        Some(result.version_id),
                    )
                }
                Err(error) => {
                    let error = error.to_string();
                    logging::error("install", &error);
                    (
                        false,
                        tf("error.install_failed", &[("error", error.as_str())]),
                        None,
                    )
                }
            }
        }
    }
}
