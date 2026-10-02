//! The server builds module in the browser preview: two server builds to install from Home; an
//! installed one is a Fabric build of the server's client (`?tensa=off` starts with Home's switch
//! off, `?tensa=error` with the server away).

use std::cell::RefCell;
use std::rc::Rc;

use launcher_shared::{AppError, AppResult, ErrorCode, Level, LoaderKind, Text};
use serde_json::{Value, json};

use crate::mock_builds::{MockBuilds, toast};

/// Id, name, loader, Minecraft, description.
type Pack = (&'static str, &'static str, LoaderKind, &'static str, Option<&'static str>);

const PACKS: [Pack; 2] = [
    (
        "aero",
        "Aero",
        LoaderKind::Fabric,
        "26.3",
        Some("Літаки, дирижаблі та небесні острови.\nСервер play.example"),
    ),
    ("tensa-lite", "Tensa Lite", LoaderKind::NeoForge, "1.21.1", None),
];

pub struct MockTensa {
    builds: Rc<RefCell<MockBuilds>>,
    show: bool,
    fails: bool,
    /// Packs installed in this preview.
    installed: Vec<&'static str>,
}

impl MockTensa {
    pub fn new(show: bool, fails: bool, builds: Rc<RefCell<MockBuilds>>) -> MockTensa {
        MockTensa { builds, show, fails, installed: Vec::new() }
    }

    fn home_packs(&self) -> AppResult<Value> {
        if !self.show {
            return Ok(json!([]));
        }
        if self.fails {
            return Err(AppError::new(ErrorCode::Network, "mock"));
        }
        let packs: Vec<Value> = PACKS
            .iter()
            .filter(|(id, ..)| !self.installed.contains(id))
            .map(|(id, name, kind, mc, description)| {
                json!({
                    "id": id, "name": name, "description": description, "image": null,
                    "runs": format!("{} {mc}", kind.display_name())
                })
            })
            .collect();
        Ok(Value::Array(packs))
    }

    fn install(&mut self, pack_id: &str) -> AppResult<Value> {
        let Some(&(id, name, kind, mc, _)) = PACKS.iter().find(|(id, ..)| *id == pack_id) else {
            return Err(AppError::new(ErrorCode::NotFound, "mock").with_param("pack", pack_id));
        };
        let mut builds = self.builds.borrow_mut();
        let build = builds.create_loader(name, kind, mc, "0.19.5")?;
        builds.set_client(&build.key, "TensaCraft");
        builds.emit_builds();
        self.installed.push(id);
        Ok(json!({"key": build.key, "name": build.name}))
    }

    /// Answers `module_invoke` for module `tensa`.
    pub fn handle(&mut self, command: &str, args: &Value) -> AppResult<Value> {
        match command {
            "home_packs" => self.home_packs(),
            "install" => self.install(args["pack_id"].as_str().unwrap_or_default()),
            "settings" => Ok(json!({"show": self.show})),
            "set_settings" => {
                self.show = args["show"].as_bool().unwrap_or(true);
                self.builds.borrow().emit_builds();
                Ok(Value::Null)
            }
            "force_sync" => {
                toast(Level::Success, Text::key("tensacraft_force_sync_complete"));
                Ok(Value::Null)
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock tensa: no {other}"))),
        }
    }
}
