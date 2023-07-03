use crate::{
    app::{App, AppExit},
    plugin::Plugin,
    UpdateFlow,
};
use bevy_ecs::{
    event::{Events, ManualEventReader},
    schedule::BoxedScheduleLabel,
};
use bevy_utils::{Duration, Instant};

#[cfg(feature = "trace")]
use bevy_utils::tracing::info_span;

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{prelude::*, JsCast};

/// Determines the method used to run an [`App`]'s [`Schedule`](bevy_ecs::schedule::Schedule).
///
/// It is used in the [`ScheduleRunnerPlugin`].
#[derive(Copy, Clone, Debug)]
pub enum RunMode {
    /// Indicates that the [`App`]'s schedule should run repeatedly.
    Loop {
        /// The minimum [`Duration`] to wait after a [`Schedule`](bevy_ecs::schedule::Schedule)
        /// has completed before repeating. A value of [`None`] will not wait.
        wait: Option<Duration>,
    },
    /// Indicates that the [`App`]'s schedule should run only once.
    Once,
}

impl Default for RunMode {
    fn default() -> Self {
        RunMode::Loop { wait: None }
    }
}

/// Configures an [`App`] to run its [`Schedule`](bevy_ecs::schedule::Schedule) according to a given
/// [`RunMode`].
///
/// [`ScheduleRunnerPlugin`] is included in the
/// [`MinimalPlugins`](https://docs.rs/bevy/latest/bevy/struct.MinimalPlugins.html) plugin group.
///
/// [`ScheduleRunnerPlugin`] is *not* included in the
/// [`DefaultPlugins`](https://docs.rs/bevy/latest/bevy/struct.DefaultPlugins.html) plugin group
/// which assumes that the [`Schedule`](bevy_ecs::schedule::Schedule) will be executed by other means:
/// typically, the `winit` event loop
/// (see [`WinitPlugin`](https://docs.rs/bevy/latest/bevy/winit/struct.WinitPlugin.html))
/// executes the schedule making [`ScheduleRunnerPlugin`] unnecessary.
pub struct ScheduleRunnerPlugin {
    /// Determines whether the [`Schedule`](bevy_ecs::schedule::Schedule) is run once or repeatedly.
    pub run_mode: RunMode,
    /// The schedule to run by default.
    ///
    /// This is initially set to [`Main`].
    pub main_schedule_label: BoxedScheduleLabel,
}

impl ScheduleRunnerPlugin {
    /// See [`RunMode::Once`].
    pub fn run_once() -> Self {
        ScheduleRunnerPlugin {
            run_mode: RunMode::Once,
            main_schedule_label: Box::new(UpdateFlow),
        }
    }

    /// See [`RunMode::Loop`].
    pub fn run_loop(wait_duration: Duration) -> Self {
        ScheduleRunnerPlugin {
            run_mode: RunMode::Loop {
                wait: Some(wait_duration),
            },
            main_schedule_label: Box::new(UpdateFlow),
        }
    }
}

impl Default for ScheduleRunnerPlugin {
    fn default() -> Self {
        ScheduleRunnerPlugin {
            run_mode: RunMode::Loop { wait: None },
            main_schedule_label: Box::new(UpdateFlow),
        }
    }
}

impl Plugin for ScheduleRunnerPlugin {
    fn build(&self, app: &mut App) {
        let run_mode = self.run_mode;
        let main_schedule_label = self.main_schedule_label.clone();
        app.set_runner(move |mut app: App| {
            // Prevent panic when schedules do not exist
            app.init_schedule(main_schedule_label.clone());

            let mut app_exit_event_reader = ManualEventReader::<AppExit>::default();
            match run_mode {
                RunMode::Once => {
                    {
                        #[cfg(feature = "trace")]
                        let _main_schedule_span =
                            info_span!("main schedule", name = ?main_schedule_label).entered();
                        app.world.run_schedule(main_schedule_label);
                    }
                    app.update_sub_apps();
                }
                RunMode::Loop { wait } => {
                    let mut tick = move |app: &mut App,
                                         wait: Option<Duration>|
                          -> Result<Option<Duration>, AppExit> {
                        let start_time = Instant::now();

                        if let Some(app_exit_events) =
                            app.world.get_resource_mut::<Events<AppExit>>()
                        {
                            if let Some(exit) = app_exit_event_reader.iter(&app_exit_events).last()
                            {
                                return Err(exit.clone());
                            }
                        }

                        {
                            #[cfg(feature = "trace")]
                            let _main_schedule_span =
                                info_span!("main schedule", name = ?main_schedule_label).entered();
                            app.world.run_schedule(&main_schedule_label);
                        }
                        app.update_sub_apps();
                        app.world.clear_trackers();

                        if let Some(app_exit_events) =
                            app.world.get_resource_mut::<Events<AppExit>>()
                        {
                            if let Some(exit) = app_exit_event_reader.iter(&app_exit_events).last()
                            {
                                return Err(exit.clone());
                            }
                        }

                        let end_time = Instant::now();

                        if let Some(wait) = wait {
                            let exe_time = end_time - start_time;
                            if exe_time < wait {
                                return Ok(Some(wait - exe_time));
                            }
                        }

                        Ok(None)
                    };

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        while let Ok(delay) = tick(&mut app, wait) {
                            if let Some(delay) = delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    #[cfg(target_arch = "wasm32")]
                    {
                        fn set_timeout(f: &Closure<dyn FnMut()>, dur: Duration) {
                            web_sys::window()
                                .unwrap()
                                .set_timeout_with_callback_and_timeout_and_arguments_0(
                                    f.as_ref().unchecked_ref(),
                                    dur.as_millis() as i32,
                                )
                                .expect("Should register `setTimeout`.");
                        }
                        let asap = Duration::from_millis(1);

                        let mut rc = Rc::new(app);
                        let f = Rc::new(RefCell::new(None));
                        let g = f.clone();

                        let c = move || {
                            let mut app = Rc::get_mut(&mut rc).unwrap();
                            let delay = tick(&mut app, wait);
                            match delay {
                                Ok(delay) => {
                                    set_timeout(f.borrow().as_ref().unwrap(), delay.unwrap_or(asap))
                                }
                                Err(_) => {}
                            }
                        };
                        *g.borrow_mut() = Some(Closure::wrap(Box::new(c) as Box<dyn FnMut()>));
                        set_timeout(g.borrow().as_ref().unwrap(), asap);
                    };
                }
            }
        });
    }
}
