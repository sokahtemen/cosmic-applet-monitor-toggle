use cosmic::app::{Core, Task};
use cosmic::iced::platform_specific::shell::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Length, Limits};
use cosmic::widget::{self, text, Column, Row};
use cosmic::{Application, Element};
use tokio::process::Command;

const APP_ID: &str = "io.github.monitor_toggle";

#[derive(Debug, Clone)]
struct Output {
    name: String,
    model: String,
    enabled: bool,
    position: Option<(i32, i32)>,
}

async fn randr(args: &[&str]) -> Result<String, String> {
    let out = Command::new("cosmic-randr")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("cosmic-randr: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parses the relevant fields from `cosmic-randr list --kdl`.
async fn list_outputs() -> Result<Vec<Output>, String> {
    let kdl = randr(&["list", "--kdl"]).await?;
    let mut outputs: Vec<Output> = Vec::new();
    for line in kdl.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("output \"") {
            let Some((name, rest)) = rest.split_once('"') else { continue };
            outputs.push(Output {
                name: name.to_string(),
                model: String::new(),
                enabled: rest.contains("enabled=#true"),
                position: None,
            });
        } else if let Some(o) = outputs.last_mut() {
            if let Some(rest) = line.strip_prefix("description ") {
                if let Some(m) = rest.split("model=\"").nth(1) {
                    o.model = m.trim_end_matches('"').to_string();
                }
            } else if let Some(rest) = line.strip_prefix("position ") {
                let mut it = rest.split_whitespace().filter_map(|n| n.parse().ok());
                if let (Some(x), Some(y)) = (it.next(), it.next()) {
                    o.position = Some((x, y));
                }
            }
        }
    }
    Ok(outputs)
}

async fn set_enabled(name: String, enable: bool, position: Option<(i32, i32)>) -> Result<Vec<Output>, String> {
    randr(&[if enable { "enable" } else { "disable" }, &name]).await?;
    // Re-enabled outputs may come back at a default position; restore the old one.
    if let (true, Some((x, y))) = (enable, position) {
        randr(&["position", &name, &x.to_string(), &y.to_string()]).await?;
    }
    list_outputs().await
}

#[derive(Default)]
struct App {
    core: Core,
    popup: Option<Id>,
    outputs: Vec<Output>,
    /// Last known positions of outputs we disabled, keyed by name.
    saved_positions: Vec<(String, (i32, i32))>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    TogglePopup,
    PopupClosed(Id),
    Set(String, bool),
    Listed(Result<Vec<Output>, String>),
}

fn refresh() -> Task<Message> {
    Task::perform(list_outputs(), |r| cosmic::Action::App(Message::Listed(r)))
}

impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        (App { core, ..Default::default() }, refresh())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Listed(Ok(outputs)) => {
                self.outputs = outputs;
                self.error = None;
            }
            Message::Listed(Err(e)) => self.error = Some(e),
            Message::Set(name, enable) => {
                let mut position = None;
                if let Some(o) = self.outputs.iter_mut().find(|o| o.name == name) {
                    if !enable {
                        if let Some(p) = o.position {
                            self.saved_positions.retain(|(n, _)| *n != name);
                            self.saved_positions.push((name.clone(), p));
                        }
                    }
                    o.enabled = enable;
                }
                if enable {
                    position = self.saved_positions.iter().find(|(n, _)| *n == name).map(|(_, p)| *p);
                }
                return Task::perform(set_enabled(name, enable, position), |r| {
                    cosmic::Action::App(Message::Listed(r))
                });
            }
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return destroy_popup(id);
                }
                let id = Id::unique();
                self.popup = Some(id);
                let mut settings = self.core.applet.get_popup_settings(
                    self.core.main_window_id().unwrap(),
                    id,
                    None,
                    None,
                    None,
                );
                settings.positioner.size_limits = Limits::NONE
                    .min_width(260.0)
                    .max_width(360.0)
                    .min_height(80.0)
                    .max_height(500.0);
                return Task::batch([get_popup(settings), refresh()]);
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        self.core
            .applet
            .icon_button("video-display-symbolic")
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let enabled_count = self.outputs.iter().filter(|o| o.enabled).count();
        let mut col = Column::new().spacing(12).padding(16).width(Length::Fill);
        col = col.push(text::title4("Monitorok"));
        for o in &self.outputs {
            // Never allow turning off the last enabled monitor.
            let locked = o.enabled && enabled_count <= 1;
            let name = o.name.clone();
            let toggler = widget::toggler(o.enabled)
                .on_toggle_maybe((!locked).then(|| move |on| Message::Set(name.clone(), on)));
            col = col.push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(8)
                    .push(
                        Column::new()
                            .push(text::body(o.model.clone()))
                            .push(text::caption(o.name.clone())),
                    )
                    .push(widget::space().width(Length::Fill))
                    .push(toggler),
            );
        }
        if let Some(e) = &self.error {
            col = col.push(text::body(format!("⚠ {e}")));
        }
        self.core.applet.popup_container(col).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<App>(())
}
