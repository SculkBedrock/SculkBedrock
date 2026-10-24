//! 调度运行插件：为主循环安装不同风格的 runner。
//!
//! [`ScheduleRunnerPlugin`] 提供「只跑一次」与「按最小间隔循环运行」两种模式，
//! 并在启动 runner 时补做尚未执行的 `finish` / `cleanup`。
//!
//! The schedule runner plugin: installs differently styled runners for the
//! main loop.
//!
//! [`ScheduleRunnerPlugin`] offers a "run once" mode and a "loop with a minimum
//! interval" mode, and performs the pending `finish` / `cleanup` when the
//! runner starts.
use crate::app::plugin::{Plugin, PluginsState};
use crate::app::{App, AppExit};
use std::time::Duration;
use tokio::time::Instant;

/// 运行模式：调度只跑一次还是循环运行。
///
/// Run mode: whether the schedule runs once or repeatedly.
#[derive(Copy, Clone, Debug)]
pub enum RunMode {
    /// 表示 [`App`] 的调度应重复运行。
    ///
    /// Indicates that the [`App`]'s schedule should run repeatedly.
    Loop {
        /// 一个[调度](crate::schedule)执行完后、重复前的最小等待
        /// [`Duration`]；为 [`None`] 时不等待。
        ///
        /// The minimum [`Duration`] to wait after a [schedule](crate::schedule)
        /// has completed before repeating. A value of [`None`] will not wait.
        wait: Option<Duration>,
    },
    /// 表示 [`App`] 的调度只运行一次。
    ///
    /// Indicates that the [`App`]'s schedule should run only once.
    Once,
}

impl Default for RunMode {
    /// 默认使用不等待的循环模式。
    ///
    /// Defaults to the loop mode without waiting.
    fn default() -> Self {
        RunMode::Loop { wait: None }
    }
}

/// 调度运行插件：为 App 安装主循环 runner。
///
/// Schedule runner plugin: installs the main-loop runner for the App.
#[derive(Default)]
pub struct ScheduleRunnerPlugin {
    /// 运行模式。
    ///
    /// The run mode.
    pub run_mode: RunMode,
}

impl ScheduleRunnerPlugin {
    /// 参见 [`RunMode::Once`]：只运行一次。
    ///
    /// See [`RunMode::Once`]: runs only once.
    pub fn run_once() -> Self {
        ScheduleRunnerPlugin {
            run_mode: RunMode::Once,
        }
    }

    /// 参见 [`RunMode::Loop`]：循环运行，可设最小间隔。
    ///
    /// See [`RunMode::Loop`]: runs repeatedly with an optional minimum
    /// interval.
    ///
    /// - `wait`: 每轮之间的最小等待时间
    pub fn run_loop(wait: Option<Duration>) -> Self {
        ScheduleRunnerPlugin {
            run_mode: RunMode::Loop { wait },
        }
    }
}

impl Plugin for ScheduleRunnerPlugin {
    /// 安装 runner：先 finish/cleanup（若未做），再按模式驱动 update。
    ///
    /// Installs the runner: finish/cleanup first (if not yet done), then
    /// drives `update` according to the mode.
    ///
    /// - `app`: 目标应用
    fn build(&self, app: &App) {
        let run_mode = self.run_mode;
        app.set_runner(move |app: &App| {
            let plugins_state = app.plugins_state();
            if plugins_state != PluginsState::Cleaned {
                app.finish();
                app.cleanup();
            }

            match run_mode {
                RunMode::Once => {
                    app.update();

                    if let Some(exit) = app.should_exit() {
                        return exit;
                    }

                    AppExit::Success
                }
                RunMode::Loop { wait } => {
                    let tick = move |app: &App,
                                     wait: Option<Duration>|
                          -> Result<Option<Duration>, AppExit> {
                        let start_time = Instant::now();

                        app.update();

                        if let Some(exit) = app.should_exit() {
                            return Err(exit);
                        };

                        let end_time = Instant::now();

                        if let Some(wait) = wait {
                            let exe_time = end_time - start_time;
                            if exe_time < wait {
                                return Ok(Some(wait - exe_time));
                            }
                        }

                        Ok(None)
                    };
                    loop {
                        match tick(&app, wait) {
                            Ok(Some(delay)) => std::thread::sleep(delay),
                            Ok(None) => continue,
                            Err(exit) => return exit,
                        }
                    }
                }
            }
        });
    }
}
