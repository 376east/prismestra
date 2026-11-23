//! Typed draft editing for the native property list. No device or OS operations.
use super::Page;
use crate::{
    app::{
        ambient,
        device_setup::{self, Layout},
        document::Document,
        lcd, sensors,
    },
    capture::ambient::Mode,
    devices::{Driver, Role},
    lighting::ambient::Rect,
};
use std::{borrow::Cow, collections::HashMap};
const SCENE_EDGES: &str = "@scene_edges";
const LEGACY_HOME_ASSISTANT_ROUTE: &str = "Сохранён старый маршрут Home Assistant. Чтобы вернуть свет в сцены, откройте «Устройства», выберите «Общий холст» и снова включите свет.";

fn can_switch_to_ambient(driver: &Driver) -> bool {
    matches!(
        driver,
        Driver::SkydimoSerial { .. } | Driver::WledDdp | Driver::OpenRgb
    )
}

#[derive(Clone, Debug)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub choices: Vec<(String, String)>,
    pub hint: Cow<'static, str>,
    pub file: bool,
}
fn field(
    key: &'static str,
    label: &'static str,
    value: impl ToString,
    hint: impl Into<Cow<'static, str>>,
) -> Field {
    Field {
        key,
        label,
        value: value.to_string(),
        choices: vec![],
        hint: hint.into(),
        file: false,
    }
}
fn boolean(key: &'static str, label: &'static str, value: bool, hint: &'static str) -> Field {
    let mut f = field(key, label, if value { "true" } else { "false" }, hint);
    f.choices = vec![("Да".into(), "true".into()), ("Нет".into(), "false".into())];
    f
}
fn parse<T: std::str::FromStr>(value: &str) -> Result<T, String> {
    value
        .trim()
        .parse()
        .map_err(|_| "Введите корректное число".into())
}
fn boolean_value(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("Выберите Да или Нет".into()),
    }
}
fn crop(value: &str) -> Result<Option<Rect>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let n = value
        .split(',')
        .map(parse)
        .collect::<Result<Vec<usize>, _>>()?;
    if n.len() != 4 {
        return Err("Обрезка: x, y, ширина, высота. Пусто - весь экран.".into());
    }
    Ok(Some(Rect {
        x: n[0],
        y: n[1],
        width: n[2],
        height: n[3],
    }))
}

fn line_layout(value: &str) -> Result<Layout, String> {
    let n = value
        .split(',')
        .map(parse)
        .collect::<Result<Vec<u32>, _>>()?;
    if n.len() != 4 {
        return Err("Линия: x0,y0,x1,y1".into());
    }
    Ok(Layout::Line {
        x0: n[0],
        y0: n[1],
        x1: n[2],
        y1: n[3],
    })
}
pub struct Editor {
    pub document: Document,
    pub page: Page,
    pub index: usize,
}
impl Editor {
    pub fn select_device(&mut self, id: &str) -> Result<(), String> {
        if self.page != Page::Devices {
            return Err("Откройте раздел «Устройства»".into());
        }
        self.index =
            Self::device_index(&self.document, id).ok_or("Устройства нет в черновике профиля")?;
        Ok(())
    }

    fn device_index(document: &Document, id: &str) -> Option<usize> {
        document
            .lighting
            .setup
            .devices
            .iter()
            .position(|device| device.id == id)
    }

    #[cfg(any(windows, test))]
    pub(super) fn selected_device_id(&self) -> Option<&str> {
        if self.page != Page::Devices {
            return None;
        }
        self.document
            .lighting
            .setup
            .devices
            .get(self.index)
            .map(|device| device.id.as_str())
    }

    #[cfg(any(windows, test))]
    pub(super) fn selected_output_id(&self) -> Option<&str> {
        match self.page {
            Page::Lcd => self
                .document
                .lcd
                .get(self.index)
                .map(|lcd| lcd.device.as_str()),
            Page::Ambient => self
                .document
                .ambient
                .routes
                .get(self.index)
                .map(|route| route.device.as_str()),
            _ => None,
        }
    }

    #[cfg(any(windows, test))]
    pub(super) fn device_index_in(&self, document: &Document) -> usize {
        self.selected_device_id()
            .and_then(|id| Self::device_index(document, id))
            .unwrap_or(document.lighting.setup.devices.len())
    }

    pub fn targets(document: &Document, page: Page) -> Vec<String> {
        match page {
            Page::Devices => document
                .lighting
                .setup
                .devices
                .iter()
                .map(|d| format!("{} ({})", d.label, d.id))
                .collect(),
            Page::Ambient => Self::output_targets(
                document,
                document.ambient.routes.iter().map(|r| r.device.as_str()),
            ),
            Page::Lcd => {
                Self::output_targets(document, document.lcd.iter().map(|r| r.device.as_str()))
            }
            Page::Temperatures => document
                .temperatures
                .sources
                .iter()
                .map(|s| s.key.clone())
                .collect(),
            _ => vec![],
        }
    }

    fn output_targets<'a>(
        document: &'a Document,
        ids: impl Iterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut counts = HashMap::new();
        let targets: Vec<_> = ids
            .map(|id| {
                let label = document
                    .lighting
                    .setup
                    .devices
                    .iter()
                    .find(|device| device.id == id)
                    .map_or(id, |device| device.label.as_str());
                *counts.entry(label).or_insert(0) += 1;
                (id, label)
            })
            .collect();
        targets
            .into_iter()
            .map(|(id, label)| {
                if counts[label] > 1 {
                    format!("{label} ({id})")
                } else {
                    label.to_owned()
                }
            })
            .collect()
    }

    #[cfg(any(windows, test))]
    pub(super) fn selected_connection_device(&self) -> Option<&str> {
        self.selected_device_id()
            .or_else(|| self.selected_output_id())
    }

    pub fn fields(&self) -> Vec<Field> {
        match self.page {
            Page::Devices => self.device_fields(),
            Page::Ambient => self.ambient_fields(),
            Page::Lcd => self.lcd_fields(),
            Page::Temperatures => self.temperature_fields(),
            _ => vec![],
        }
    }
    fn device_fields(&self) -> Vec<Field> {
        let Some(d) = self.document.lighting.setup.devices.get(self.index) else {
            return vec![];
        };
        let current = device_setup::catalog()
            .iter()
            .find(|p| {
                p.device(&d.id)
                    .is_ok_and(|v| v.driver == d.driver && v.role == d.role && v.leds == d.leds)
            })
            .map_or("current", |p| p.id);
        let mut preset = field(
            "preset",
            "Семейство",
            current,
            "Смена числа LED требует команды replace_line. Новое семейство отключено.",
        );
        preset.choices = vec![("Текущее".into(), "current".into())];
        preset.choices.extend(
            device_setup::catalog()
                .iter()
                .map(|p| (p.label.into(), p.id.into())),
        );
        let mut group = field(
            "group",
            "Группа",
            if device_setup::target(&self.document, &d.id) == crate::schedule::Target::Pc {
                "pc"
            } else {
                "door"
            },
            "Устройства одной группы можно включать и выключать вместе.",
        );
        group.choices = vec![
            ("Основной свет".into(), "pc".into()),
            ("Дополнительный свет".into(), "door".into()),
        ];
        let mut fields = vec![
            field("name", "Название", &d.label, "Название в профиле"),
            boolean(
                "enabled",
                "Включено",
                d.enabled,
                "Разрешение в профиле; само по себе не подключает оборудование.",
            ),
            field(
                "rate",
                "Частота",
                d.rate_hz,
                match d.driver {
                    crate::devices::Driver::HomeAssistant => {
                        "Home Assistant: один светильник, 1 Гц"
                    }
                    crate::devices::Driver::WledDdp => {
                        "WLED: 1..60 Гц; максимум 1 МБ RGB/с на выход"
                    }
                    crate::devices::Driver::OpenRgb => {
                        "OpenRGB: 1..60 Гц; предел зависит от контроллера"
                    }
                    _ => "1..144 Гц",
                },
            ),
            group,
            preset,
        ];
        let legacy_home_assistant = d.driver == Driver::HomeAssistant && d.role == Role::Ambient;
        if can_switch_to_ambient(&d.driver) || legacy_home_assistant {
            let mut role = field(
                "role",
                "Назначение",
                if d.role == Role::Ambient {
                    "ambient"
                } else {
                    "canvas"
                },
                if legacy_home_assistant {
                    LEGACY_HOME_ASSISTANT_ROUTE
                } else {
                    "Смена назначения выключает ленту и убирает прежнюю карту или связь. Для холста создаётся линия."
                },
            );
            role.choices = vec![
                ("Общий холст".into(), "canvas".into()),
                (
                    if legacy_home_assistant {
                        "От экрана (старый маршрут)"
                    } else {
                        "От экрана"
                    }
                    .into(),
                    "ambient".into(),
                ),
            ];
            fields.insert(2, role);
        }
        if d.role == crate::devices::Role::Canvas {
            fields.extend([
                field(
                    "layout_line",
                    "Разместить линией",
                    "",
                    "x0,y0,x1,y1; явная замена карты",
                ),
                field(
                    "layout_grid",
                    "Разместить сеткой",
                    "",
                    "x,y,width,height,columns,serpentine (true/false)",
                ),
                field(
                    "point",
                    "Переместить LED",
                    "",
                    "index,x,y; индекс LED с нуля",
                ),
            ]);
            fields.push(field(
                "leds_line",
                "Число LED и карта",
                "",
                "count;x0,y0,x1,y1; только допустимое драйвером число LED",
            ));
        }
        fields.push(field(
            "replace_line",
            "Заменить с картой",
            "",
            "preset_id;x0,y0,x1,y1; новое семейство отключено",
        ));
        if let crate::devices::Driver::Fusion2 { headers } = d.driver {
            fields.push(field(
                "headers",
                "LED заголовков",
                format!("{},{}", headers[0], headers[1]),
                "1..256,1..256; изменение суммы требует headers_line.",
            ));
            fields.push(field(
                "headers_line",
                "Заголовки и карта",
                "",
                "N,N;x0,y0,x1,y1; явная замена карты",
            ));
        }
        fields
    }
    pub fn add_device(&mut self, preset: &str) -> Result<(), String> {
        let d = device_setup::preset(preset)?.device("temporary")?;
        self.add_device_with_leds(preset, d.leds)
    }
    pub fn add_device_with_leds(&mut self, preset: &str, leds: usize) -> Result<(), String> {
        self.document.validate()?;
        let p = device_setup::preset(preset)?;
        // Local bindings outlive the editor and can keep IDs of removed devices.
        // A newly added device must never adopt one of those assignments.
        let mut identity = [0; 16];
        getrandom::fill(&mut identity).map_err(|_| "Не удалось создать ID устройства")?;
        let id = format!("device-{:032x}", u128::from_be_bytes(identity));
        let d = p.device_with_leds(&id, leds)?;
        let c = &self.document.lighting.setup.canvas;
        let layout = (d.role == crate::devices::Role::Canvas).then_some(Layout::Line {
            x0: 0,
            y0: (self.document.lighting.setup.devices.len() as u32) % c.height,
            x1: c.width - 1,
            y1: (self.document.lighting.setup.devices.len() as u32) % c.height,
        });
        self.document = device_setup::add(&self.document, d, p.target(), layout)?;
        self.index = self.document.lighting.setup.devices.len() - 1;
        Ok(())
    }
    pub fn remove_device(&mut self) -> Result<(), String> {
        let id = self
            .document
            .lighting
            .setup
            .devices
            .get(self.index)
            .ok_or("Выберите устройство")?
            .id
            .clone();
        self.document = device_setup::remove(&self.document, &id)?;
        self.index = self.document.lighting.setup.devices.len();
        Ok(())
    }
    /// Only logical, unrouted ambient devices. No discovery or native I/O.
    pub fn ambient_candidates(&self) -> Vec<(String, String)> {
        self.document
            .lighting
            .setup
            .devices
            .iter()
            .filter(|d| d.role == Role::Ambient && d.driver != Driver::HomeAssistant)
            .filter(|d| {
                !self
                    .document
                    .ambient
                    .routes
                    .iter()
                    .any(|r| r.device == d.id)
            })
            .map(|d| (d.id.clone(), format!("{} ({})", d.label, d.id)))
            .collect()
    }
    pub fn add_ambient_route(&mut self, device: &str, source: Option<&str>) -> Result<(), String> {
        if self.page != Page::Ambient {
            return Err("Откройте экранную подсветку".into());
        }
        self.document.validate()?;
        let d = self
            .document
            .lighting
            .setup
            .devices
            .iter()
            .find(|d| d.id == device && d.role == Role::Ambient)
            .ok_or("Выберите устройство экранной подсветки")?;
        if d.driver == Driver::HomeAssistant {
            return Err("Для Home Assistant доступны сцены через «Общий холст».".into());
        }
        if self
            .document
            .ambient
            .routes
            .iter()
            .any(|r| r.device == device)
        {
            return Err("Устройство уже связано с экраном".into());
        }
        let mut next = self.document.clone();
        let source = if let Some(key) = source {
            if !next.ambient.sources.iter().any(|s| s.key == key) {
                return Err("Выберите существующий источник экрана".into());
            }
            key.to_owned()
        } else {
            if next.ambient.sources.len() >= 16 {
                return Err("Уже есть 16 источников. Выберите существующий.".into());
            }
            let key = (1..=17)
                .map(|n| format!("screen-{n}"))
                .find(|key| !next.ambient.sources.iter().any(|s| s.key == *key))
                .ok_or("Нет свободного имени источника")?;
            next.ambient.sources.push(ambient::Source {
                key: key.clone(),
                rate_hz: d.rate_hz,
                crop: None,
                stale_after_ms: 1000,
            });
            key
        };
        next.ambient.routes.push(ambient::Route {
            device: device.into(),
            source,
            scene_edges: false,
            enabled: false,
            zone: crate::capture::ambient::Zone::linear(d.leds, false, Mode::ScreenCopy)?,
            processing: Default::default(),
            output: Default::default(),
        });
        next.validate()?;
        self.index = next.ambient.routes.len() - 1;
        self.document = next;
        Ok(())
    }

    #[cfg(any(windows, test))]
    pub(super) fn ambient_route_notice(&self) -> Option<&'static str> {
        let id = self
            .document
            .ambient
            .routes
            .get(self.index)?
            .device
            .as_str();
        self.document
            .lighting
            .setup
            .devices
            .iter()
            .any(|d| d.id == id && d.driver == Driver::HomeAssistant)
            .then_some(LEGACY_HOME_ASSISTANT_ROUTE)
    }

    /// Removing a route keeps its source (possibly shared) and the device.
    /// Disable the now-unrouted output in this draft to retain a valid profile.
    pub fn remove_ambient_route(&mut self) -> Result<(), String> {
        if self.page != Page::Ambient {
            return Err("Откройте экранную подсветку".into());
        }
        self.document.validate()?;
        let mut next = self.document.clone();
        let id = next
            .ambient
            .routes
            .get(self.index)
            .ok_or("Выберите связь с экраном")?
            .device
            .clone();
        next.ambient.routes.remove(self.index);
        let d = next
            .lighting
            .setup
            .devices
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or("Устройство не найдено")?;
        d.enabled = false;
        next.validate()?;
        self.index = self.index.min(next.ambient.routes.len().saturating_sub(1));
        self.document = next;
        Ok(())
    }
    pub fn add_lcd_device(&mut self) -> Result<(), String> {
        if self.page != Page::Lcd {
            return Err("Откройте LCD".into());
        }
        self.add_device("tt-lcd")?;
        let id = &self.document.lighting.setup.devices[self.index].id;
        self.index = self
            .document
            .lcd
            .iter()
            .position(|lcd| lcd.device == *id)
            .ok_or("LCD не добавлен")?;
        Ok(())
    }
    pub fn replace_device_with_layout(
        &mut self,
        preset: &str,
        layout: Layout,
    ) -> Result<(), String> {
        self.replace_preset(preset, Some(layout))
    }
    fn replace_preset(&mut self, preset: &str, layout: Option<Layout>) -> Result<(), String> {
        let old = self
            .document
            .lighting
            .setup
            .devices
            .get(self.index)
            .ok_or("Выберите устройство")?;
        let mut d = device_setup::preset(preset)?.device(&old.id)?;
        d.label = old.label.clone();
        d.rate_hz = old.rate_hz;
        if d.driver == old.driver && d.leds == old.leds && d.role == old.role {
            d.enabled = old.enabled;
        }
        self.document = device_setup::replace(
            &self.document,
            &old.id,
            d,
            device_setup::target(&self.document, &old.id),
            layout,
        )?;
        Ok(())
    }
    pub fn replace_headers_with_layout(
        &mut self,
        headers: [usize; 2],
        layout: Layout,
    ) -> Result<(), String> {
        let mut d = self
            .document
            .lighting
            .setup
            .devices
            .get(self.index)
            .ok_or("Выберите устройство")?
            .clone();
        if !matches!(d.driver, crate::devices::Driver::Fusion2 { .. }) {
            return Err("Выберите Fusion2".into());
        }
        d.leds = headers[0]
            .checked_add(headers[1])
            .ok_or("Переполнение числа LED")?;
        d.driver = crate::devices::Driver::Fusion2 { headers };
        self.document = device_setup::replace(
            &self.document,
            &d.id.clone(),
            d,
            device_setup::target(
                &self.document,
                &self.document.lighting.setup.devices[self.index].id,
            ),
            Some(layout),
        )?;
        Ok(())
    }
    fn set_device(&mut self, key: &str, value: &str) -> Result<(), String> {
        let mut d = self
            .document
            .lighting
            .setup
            .devices
            .get(self.index)
            .ok_or("Выберите устройство")?
            .clone();
        let mut target = device_setup::target(&self.document, &d.id);
        match key {
            "role" => {
                let legacy_home_assistant =
                    d.driver == Driver::HomeAssistant && d.role == Role::Ambient;
                if !can_switch_to_ambient(&d.driver) && !legacy_home_assistant {
                    return Err("Для этого семейства нельзя сменить назначение".into());
                }
                let role = match value {
                    "canvas" => crate::devices::Role::Canvas,
                    "ambient" => crate::devices::Role::Ambient,
                    _ => return Err("Выберите назначение ленты".into()),
                };
                if d.role == role {
                    return Ok(());
                }
                d.role = role;
                d.enabled = false;
                let c = &self.document.lighting.setup.canvas;
                let layout = (role == crate::devices::Role::Canvas).then_some(Layout::Line {
                    x0: 0,
                    y0: self.index as u32 % c.height,
                    x1: c.width - 1,
                    y1: self.index as u32 % c.height,
                });
                self.document =
                    device_setup::replace(&self.document, &d.id.clone(), d, target, layout)?;
                return Ok(());
            }
            "leds_line" => {
                let (count, line) = value
                    .split_once(';')
                    .ok_or("Число LED и карта: count;x0,y0,x1,y1")?;
                d.leds = parse(count)?;
                self.document = device_setup::replace(
                    &self.document,
                    &d.id.clone(),
                    d,
                    target,
                    Some(line_layout(line)?),
                )?;
                return Ok(());
            }
            "layout_line" => {
                self.document = device_setup::relayout(&self.document, &d.id, line_layout(value)?)?;
                return Ok(());
            }
            "layout_grid" => {
                let v = value.split(',').map(str::trim).collect::<Vec<_>>();
                if v.len() != 6 {
                    return Err("Сетка: x,y,width,height,columns,serpentine".into());
                }
                self.document = device_setup::relayout(
                    &self.document,
                    &d.id,
                    Layout::Grid {
                        x: parse(v[0])?,
                        y: parse(v[1])?,
                        width: parse(v[2])?,
                        height: parse(v[3])?,
                        columns: parse(v[4])?,
                        serpentine: boolean_value(v[5])?,
                    },
                )?;
                return Ok(());
            }
            "point" => {
                let v = value
                    .split(',')
                    .map(parse)
                    .collect::<Result<Vec<usize>, _>>()?;
                if v.len() != 3 {
                    return Err("Точка: index,x,y".into());
                }
                self.document = device_setup::move_point(
                    &self.document,
                    &d.id,
                    v[0],
                    u32::try_from(v[1]).map_err(|_| "Координата слишком велика")?,
                    u32::try_from(v[2]).map_err(|_| "Координата слишком велика")?,
                )?;
                return Ok(());
            }
            "replace_line" => {
                let (p, line) = value
                    .split_once(';')
                    .ok_or("Замена: preset_id;x0,y0,x1,y1")?;
                return self.replace_device_with_layout(p.trim(), line_layout(line)?);
            }
            "preset" => {
                return if value == "current" {
                    Ok(())
                } else {
                    self.replace_preset(value, None)
                };
            }
            "name" => d.label = value.into(),
            "enabled" => d.enabled = boolean_value(value)?,
            "rate" => d.rate_hz = parse(value)?,
            "group" => {
                target = match value {
                    "pc" => crate::schedule::Target::Pc,
                    "door" => crate::schedule::Target::Door,
                    _ => return Err("Выберите основной или дополнительный свет".into()),
                }
            }
            "headers_line" => {
                let (counts, line) = value
                    .split_once(';')
                    .ok_or("Заголовки и карта: N,N;x0,y0,x1,y1")?;
                let n = counts
                    .split(',')
                    .map(parse)
                    .collect::<Result<Vec<usize>, _>>()?;
                if n.len() != 2 {
                    return Err("Два числа LED через запятую".into());
                }
                return self.replace_headers_with_layout([n[0], n[1]], line_layout(line)?);
            }
            "headers" => {
                if !matches!(d.driver, crate::devices::Driver::Fusion2 { .. }) {
                    return Err("Выберите Fusion2".into());
                }
                let n = value
                    .split(',')
                    .map(parse)
                    .collect::<Result<Vec<usize>, _>>()?;
                if n.len() != 2 {
                    return Err("Два числа LED через запятую".into());
                }
                d.leds = n[0].checked_add(n[1]).ok_or("Переполнение числа LED")?;
                d.driver = crate::devices::Driver::Fusion2 {
                    headers: [n[0], n[1]],
                };
            }
            _ => return Err("Параметр недоступен".into()),
        }
        self.document = device_setup::replace(&self.document, &d.id.clone(), d, target, None)?;
        Ok(())
    }
    fn ambient_fields(&self) -> Vec<Field> {
        let Some(r) = self.document.ambient.routes.get(self.index) else {
            return vec![];
        };
        let Some(s) = self
            .document
            .ambient
            .sources
            .iter()
            .find(|s| s.key == r.source)
        else {
            return vec![];
        };
        let mut source = field(
            "source",
            "Источник",
            if r.scene_edges {
                SCENE_EDGES
            } else {
                &r.source
            },
            "Края общей сцены идут от верхнего левого угла по часовой стрелке. Этот источник работает без захвата монитора.",
        );
        source.choices = self
            .document
            .ambient
            .sources
            .iter()
            .map(|s| (s.key.clone(), s.key.clone()))
            .collect();
        source
            .choices
            .push(("Края общей сцены".into(), SCENE_EDGES.into()));
        let mut mode = field(
            "mode",
            "Режим",
            if r.zone.mode == Mode::ScreenCopy {
                "screen_copy"
            } else {
                "average"
            },
            "Копирование экрана или один усреднённый цвет.",
        );
        mode.choices = vec![
            ("Копирование экрана".into(), "screen_copy".into()),
            ("Средний цвет".into(), "average".into()),
        ];
        let rate = self
            .document
            .lighting
            .setup
            .devices
            .iter()
            .find(|d| d.id == r.device)
            .unwrap()
            .rate_hz;
        let mut fields = vec![
            boolean(
                "enabled",
                "Включено",
                r.enabled
                    && self
                        .document
                        .lighting
                        .setup
                        .devices
                        .iter()
                        .any(|d| d.id == r.device && d.enabled),
                "Включает или выключает связь и выход на ленту в профиле.",
            ),
            source,
            mode,
            field(
                "output_rate",
                "Частота подсветки, Гц",
                rate,
                "От 1 до 144. Фактическая частота зависит от устройства.",
            ),
            field(
                "capture_rate",
                "Частота захвата, Гц",
                s.rate_hz,
                "От 1 до 144. Общая для устройств этого источника.",
            ),
            field(
                "brightness",
                "Яркость, %",
                r.output.brightness * 100.,
                "От 0 до 100.",
            ),
            field(
                "smoothness",
                "Сглаживание, %",
                r.output.smoothness,
                "От 0 до 100. Больше - медленнее изменение цвета.",
            ),
            boolean(
                "reverse",
                "Развернуть направление",
                r.zone.reverse,
                "Зеркально разворачивает горизонтальную карту светодиодов.",
            ),
            field(
                "gamma",
                "Гамма",
                r.processing.correction.gamma,
                "От 0.1 до 4.",
            ),
            field(
                "saturation",
                "Насыщенность",
                r.processing.correction.saturation,
                "От 0 до 3; 1 - исходная насыщенность.",
            ),
            field(
                "kelvin",
                "Белый, К",
                r.processing.correction.kelvin,
                "От 2000 до 10000; 6500 - нейтральный белый.",
            ),
            field(
                "gain_r",
                "Красный канал",
                r.processing.correction.gains[0],
                "Множитель от 0 до 2.",
            ),
            field(
                "gain_g",
                "Зелёный канал",
                r.processing.correction.gains[1],
                "Множитель от 0 до 2.",
            ),
            field(
                "gain_b",
                "Синий канал",
                r.processing.correction.gains[2],
                "Множитель от 0 до 2.",
            ),
            field(
                "temperature",
                "Тёплый / холодный",
                r.output.temperature,
                "От -100 до 100.",
            ),
            field("tint", "Оттенок", r.output.tint, "От -100 до 100."),
            field("blur", "Размытие", r.processing.blur, "От 0 до 10."),
            boolean(
                "borders",
                "Определять чёрные полосы",
                r.processing.borders.enabled,
                "Автоматически исключает устойчивые чёрные края.",
            ),
            field(
                "threshold",
                "Порог чёрного, %",
                r.processing.borders.black_threshold,
                "От 0 до 100.",
            ),
            field(
                "stable",
                "Подтверждение полос, кадров",
                r.processing.borders.stable_frames,
                "От 1 до 200.",
            ),
            field(
                "unknown",
                "Сброс полос, кадров",
                r.processing.borders.unknown_frames,
                "От 1 до 2000 при полностью чёрном кадре.",
            ),
            field(
                "crop",
                "Обрезка",
                s.crop
                    .map(|r| format!("{}, {}, {}, {}", r.x, r.y, r.width, r.height))
                    .unwrap_or_default(),
                "x, y, ширина, высота; пусто - весь экран. Общая для источника.",
            ),
            field(
                "stale",
                "Гасить без сигнала через, мс",
                s.stale_after_ms,
                "От 100 до 60000. Старый кадр не остаётся навсегда.",
            ),
        ];
        if r.scene_edges {
            fields.retain(|f| matches!(f.key, "enabled" | "source" | "output_rate" | "reverse"));
        }
        fields
    }
    fn lcd_fields(&self) -> Vec<Field> {
        let Some(entry) = self.document.lcd.get(self.index) else {
            return vec![];
        };
        let c = &entry.config;
        let mut media = field(
            "media",
            "Фон LCD",
            c.media
                .as_ref()
                .map(|id| format!("Выбран · {}", &id.as_str()[..8]))
                .unwrap_or_default(),
            "Изображение или анимация до 32 МиБ. Очистите поле, чтобы оставить температуру на чёрном фоне.",
        );
        media.file = true;
        let mut font = field(
            "font",
            "Шрифт панели",
            c.font
                .as_ref()
                .map(|id| format!("Выбран · {}", &id.as_str()[..8]))
                .unwrap_or_else(|| "Встроенный".into()),
            "Можно выбрать TTF/OTF до 8 МиБ. По умолчанию используется встроенный шрифт.",
        );
        font.file = true;
        let mut rotation = field(
            "rotation",
            "Поворот, °",
            c.rotation.rem_euclid(360),
            "Поворот экрана без изменения читаемости панели.",
        );
        rotation.choices = [0, 90, 180, 270]
            .into_iter()
            .map(|n| (n.to_string(), n.to_string()))
            .collect();
        let styles = crate::lcd::overlay::GaugeStyle::OPTIONS;
        let key = styles
            .iter()
            .find(|(style, _, _)| *style == c.gauge_style)
            .unwrap()
            .1;
        let mut gauge = field(
            "gauge_style",
            "Вид показаний",
            key,
            "Шкала, столбик и пламя меняются с температурой.",
        );
        gauge.choices = styles
            .into_iter()
            .map(|(_, key, label)| (label.into(), key.into()))
            .collect();
        let sources = &self.document.temperatures.sources;
        let source_hint: Cow<'static, str> = if sources.is_empty() {
            "Сначала добавьте источник в разделе «Температуры».".into()
        } else {
            let example = sources
                .iter()
                .take(2)
                .map(|source| format!("{}={}", source.key, source.key.to_ascii_uppercase()))
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "Слева ключ из раздела «Температуры», справа подпись на LCD. Разделяйте показатели точкой с запятой. В этом профиле можно ввести «{example}»."
            )
            .into()
        };
        let mut fields = vec![
            boolean(
                "enabled",
                "Воспроизводить",
                c.enabled,
                "Выберите фон или включите панель температур.",
            ),
            media,
            rotation,
            boolean(
                "overlay",
                "Панель температур",
                c.overlay_enabled,
                "Показания выводятся только пока они свежие.",
            ),
            gauge,
            field(
                "card_position",
                "Позиция карточки",
                c.card_position
                    .map(|p| format!("{}, {}", p.x, p.y))
                    .unwrap_or_default(),
                "Укажите X от 0 до 292 и Y от 0 до 404 через запятую. Пустое поле оставляет стандартную позицию.",
            ),
            font,
            field(
                "interval",
                "Менять показатель через, с",
                c.interval_seconds,
                "От 2 до 60.",
            ),
            field(
                "sources",
                "Показатели и подписи",
                c.sources
                    .iter()
                    .map(|s| format!("{}={}", s.key, s.label))
                    .collect::<Vec<_>>()
                    .join("; "),
                source_hint,
            ),
        ];
        if !sources.is_empty() {
            let mut add = field(
                "add_source",
                "Добавить показатель",
                "",
                "Выбор добавит показатель в черновик. Подпись можно изменить в списке выше.",
            );
            let available = sources
                .iter()
                .filter(|source| !c.sources.iter().any(|entry| entry.key == source.key));
            let placeholder = if c.sources.len() >= lcd::MAX_SOURCES {
                "Достигнут предел показателей"
            } else if available.clone().next().is_none() {
                "Все источники добавлены"
            } else {
                "Выберите источник…"
            };
            add.choices.push((placeholder.into(), String::new()));
            if c.sources.len() < lcd::MAX_SOURCES {
                add.choices
                    .extend(available.map(|source| (source.key.clone(), source.key.clone())));
            }
            fields.push(add);
        }
        fields
    }
    fn temperature_fields(&self) -> Vec<Field> {
        let Some(s) = self.document.temperatures.sources.get(self.index) else {
            return vec![];
        };
        let mut kind = field(
            "kind",
            "Тип",
            if matches!(s.value, sensors::Value::Sensor { .. }) {
                "sensor"
            } else {
                "mean"
            },
            "Прямое показание или среднее всех выбранных свежих датчиков.",
        );
        kind.choices = vec![
            ("Датчик".into(), "sensor".into()),
            ("Среднее".into(), "mean".into()),
        ];
        let mut values = vec![
            field(
                "key",
                "Название в профиле",
                &s.key,
                "Латинские буквы, цифры, дефис и подчёркивание, до 64 знаков.",
            ),
            kind,
        ];
        match &s.value {sensors::Value::Sensor{input}=>values.push(field("input","Вход показаний",input,"Имя подключаемого входа датчика. Без связи с провайдером показаний не будет.")),
            sensors::Value::Mean{members}=>values.push(field("members","Датчики среднего",members.join(", "),"Имена прямых датчиков через запятую. Среднее скрыто, если хотя бы один недоступен."))};
        values
    }
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match self.page {
            Page::Devices => self.set_device(key, value),
            Page::Ambient => self.set_ambient(key, value),
            Page::Lcd => self.set_lcd(key, value),
            Page::Temperatures => self.set_temperature(key, value),
            _ => Err("Параметр недоступен".into()),
        }
    }
    fn set_ambient(&mut self, key: &str, value: &str) -> Result<(), String> {
        let r = self
            .document
            .ambient
            .routes
            .get_mut(self.index)
            .ok_or("Выберите устройство")?;
        if key == "enabled" {
            let enabled = boolean_value(value)?;
            let device = self
                .document
                .lighting
                .setup
                .devices
                .iter_mut()
                .find(|d| d.id == r.device)
                .ok_or("Устройство не найдено")?;
            // Preserve a loaded partial state when round-tripping its displayed
            // value. An explicit toggle changes both desired gates atomically.
            if enabled != (r.enabled && device.enabled) {
                r.enabled = enabled;
                device.enabled = enabled;
            }
            return Ok(());
        }
        if key == "source" {
            if value == SCENE_EDGES {
                r.scene_edges = true;
                return Ok(());
            }
            if !self.document.ambient.sources.iter().any(|s| s.key == value) {
                return Err("Неизвестный источник".into());
            }
            r.source = value.into();
            r.scene_edges = false;
            return Ok(());
        }
        let s = self
            .document
            .ambient
            .sources
            .iter_mut()
            .find(|s| s.key == r.source)
            .ok_or("Нет источника")?;
        match key {
            "mode" => {
                r.zone.mode = match value {
                    "screen_copy" => Mode::ScreenCopy,
                    "average" => Mode::Average,
                    _ => return Err("Выберите режим".into()),
                }
            }
            "output_rate" => {
                self.document
                    .lighting
                    .setup
                    .devices
                    .iter_mut()
                    .find(|d| d.id == r.device)
                    .unwrap()
                    .rate_hz = parse(value)?
            }
            "capture_rate" => s.rate_hz = parse(value)?,
            "brightness" => r.output.brightness = parse::<f32>(value)? / 100.,
            "smoothness" => r.output.smoothness = parse(value)?,
            "reverse" => r.zone.reverse = boolean_value(value)?,
            "gamma" => r.processing.correction.gamma = parse(value)?,
            "saturation" => r.processing.correction.saturation = parse(value)?,
            "kelvin" => r.processing.correction.kelvin = parse(value)?,
            "gain_r" => r.processing.correction.gains[0] = parse(value)?,
            "gain_g" => r.processing.correction.gains[1] = parse(value)?,
            "gain_b" => r.processing.correction.gains[2] = parse(value)?,
            "temperature" => r.output.temperature = parse(value)?,
            "tint" => r.output.tint = parse(value)?,
            "blur" => r.processing.blur = parse(value)?,
            "borders" => r.processing.borders.enabled = boolean_value(value)?,
            "threshold" => r.processing.borders.black_threshold = parse(value)?,
            "stable" => r.processing.borders.stable_frames = parse(value)?,
            "unknown" => r.processing.borders.unknown_frames = parse(value)?,
            "crop" => s.crop = crop(value)?,
            "stale" => s.stale_after_ms = parse(value)?,
            _ => return Err("Неизвестный параметр".into()),
        };
        Ok(())
    }
    fn set_lcd(&mut self, key: &str, value: &str) -> Result<(), String> {
        let entry = self
            .document
            .lcd
            .get_mut(self.index)
            .ok_or("Выберите LCD")?;
        let c = &mut entry.config;
        match key {
            "enabled" => {
                let on = boolean_value(value)?;
                c.enabled = on;
                self.document
                    .lighting
                    .setup
                    .devices
                    .iter_mut()
                    .find(|d| d.id == entry.device)
                    .ok_or("LCD отсутствует в схеме")?
                    .enabled = on;
            }
            "rotation" => c.rotation = parse(value)?,
            "overlay" => c.overlay_enabled = boolean_value(value)?,
            "card_position" => {
                c.card_position = if value.trim().is_empty() {
                    None
                } else {
                    let (x, y) = value
                        .split_once(',')
                        .ok_or("Введите X и Y через запятую.")?;
                    Some(crate::lcd::overlay::CardPosition {
                        x: parse(x)?,
                        y: parse(y)?,
                    })
                };
            }
            "media" if value.trim().is_empty() => c.media = None,
            "gauge_style" => {
                c.gauge_style = crate::lcd::overlay::GaugeStyle::OPTIONS
                    .into_iter()
                    .find(|(_, key, _)| *key == value)
                    .ok_or("Неизвестный вид показаний")?
                    .0;
            }
            "interval" => c.interval_seconds = parse(value)?,
            "add_source" => {
                if value.is_empty() {
                    return Err("Выберите температурный источник.".into());
                }
                if !self
                    .document
                    .temperatures
                    .sources
                    .iter()
                    .any(|source| source.key == value)
                {
                    return Err("Температурный источник отсутствует в профиле.".into());
                }
                if !c.sources.iter().any(|source| source.key == value) {
                    if c.sources.len() >= lcd::MAX_SOURCES {
                        return Err("Достигнут предел показателей LCD.".into());
                    }
                    c.sources.push(lcd::OverlaySource {
                        key: value.into(),
                        label: value.to_ascii_uppercase(),
                    });
                }
            }
            "sources" => {
                let sources = value
                    .split(';')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| {
                        let (key, label) = s.split_once('=').ok_or("Показатель: имя=подпись")?;
                        Ok(lcd::OverlaySource {
                            key: key.trim().into(),
                            label: label.trim().into(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                c.sources = sources;
            }
            _ => return Err("Выберите файл кнопкой или известный параметр".into()),
        };
        Ok(())
    }
    fn set_temperature(&mut self, key: &str, value: &str) -> Result<(), String> {
        let source = self
            .document
            .temperatures
            .sources
            .get(self.index)
            .ok_or("Выберите источник")?;
        if key == "key" {
            if !ambient::key(value)
                || self
                    .document
                    .temperatures
                    .sources
                    .iter()
                    .enumerate()
                    .any(|(i, s)| i != self.index && s.key == value)
            {
                return Err("Имя должно быть уникальным: латинские буквы, цифры, - или _".into());
            }
            let old = source.key.clone();
            self.document.temperatures.sources[self.index].key = value.into();
            for s in &mut self.document.temperatures.sources {
                if let sensors::Value::Mean { members } = &mut s.value {
                    for m in members {
                        if *m == old {
                            *m = value.into();
                        }
                    }
                }
            }
            for l in &mut self.document.lcd {
                for s in &mut l.config.sources {
                    if s.key == old {
                        s.key = value.into();
                    }
                }
            }
            return Ok(());
        }
        let s = &mut self.document.temperatures.sources[self.index];
        match key {
            "kind" => {
                if (value == "sensor" && matches!(s.value, sensors::Value::Sensor { .. }))
                    || (value == "mean" && matches!(s.value, sensors::Value::Mean { .. }))
                {
                    return Ok(());
                }
                s.value = match value {
                    "sensor" => sensors::Value::Sensor {
                        input: s.key.clone(),
                    },
                    "mean" => sensors::Value::Mean { members: vec![] },
                    _ => return Err("Выберите тип источника".into()),
                };
            }
            "input" => {
                if !ambient::key(value) {
                    return Err("Недопустимое имя входа".into());
                }
                s.value = sensors::Value::Sensor {
                    input: value.into(),
                };
            }
            "members" => {
                s.value = sensors::Value::Mean {
                    members: value
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                }
            }
            _ => return Err("Неизвестный параметр".into()),
        };
        Ok(())
    }
    pub fn merged(&self, current: &Document) -> Result<Document, String> {
        let mut next = self.document.clone();
        next.lighting.scene = current.lighting.scene.clone();
        next.lighting.schedule = current.lighting.schedule.clone();
        next.validate()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{
            config::{Groups, Project},
            document::Lcd,
        },
        capture::ambient::{OutputSettings, Settings, Zone},
        devices::{Canvas, Device, Driver, Role, Setup},
        schedule,
    };
    fn document() -> Document {
        Document {
            version: 1,
            lighting: Project {
                version: 1,
                setup: Setup {
                    version: 1,
                    devices: vec![
                        Device {
                            id: "screen".into(),
                            label: "Экранная лента".into(),
                            role: Role::Ambient,
                            driver: Driver::Unsupported {
                                protocol: "stand".into(),
                            },
                            leds: 2,
                            rate_hz: 48,
                            enabled: false,
                        },
                        Device {
                            id: "lcd".into(),
                            label: "Помпа".into(),
                            role: Role::Lcd,
                            driver: Driver::TtLcd,
                            leds: 0,
                            rate_hz: 25,
                            enabled: false,
                        },
                    ],
                    canvas: Canvas {
                        width: 1,
                        height: 1,
                        points: vec![],
                    },
                },
                scene: crate::runtime::scene::example(),
                schedule: schedule::Config::default(),
                groups: Groups {
                    pc: vec!["screen".into(), "lcd".into()],
                    door: vec![],
                },
            },
            ambient: ambient::Config {
                sources: vec![ambient::Source {
                    key: "main".into(),
                    rate_hz: 60,
                    crop: None,
                    stale_after_ms: 2000,
                }],
                routes: vec![ambient::Route {
                    device: "screen".into(),
                    source: "main".into(),
                    scene_edges: false,
                    enabled: true,
                    zone: Zone::linear(2, false, Mode::ScreenCopy).unwrap(),
                    processing: Settings::default(),
                    output: OutputSettings::default(),
                }],
            },
            lcd: vec![Lcd {
                device: "lcd".into(),
                config: lcd::Config {
                    sources: vec![lcd::OverlaySource {
                        key: "ram".into(),
                        label: "RAM".into(),
                    }],
                    ..Default::default()
                },
            }],
            temperatures: sensors::Config {
                sources: vec![
                    sensors::Source {
                        key: "a".into(),
                        value: sensors::Value::Sensor {
                            input: "slot-a".into(),
                        },
                    },
                    sensors::Source {
                        key: "b".into(),
                        value: sensors::Value::Sensor {
                            input: "slot-b".into(),
                        },
                    },
                    sensors::Source {
                        key: "ram".into(),
                        value: sensors::Value::Mean {
                            members: vec!["a".into(), "b".into()],
                        },
                    },
                ],
            },
        }
    }
    #[test]
    fn ambient_toggle_controls_both_gates_and_partial_roundtrip_is_lossless() {
        let mut e = Editor {
            document: document(),
            page: Page::Ambient,
            index: 0,
        };
        e.document.lighting.setup.devices[0].driver = Driver::SkydimoSerial {
            model: "SK0410".into(),
        };
        let before = e.document.clone(); // route enabled, output disabled
        assert_eq!(
            e.fields()
                .iter()
                .find(|f| f.key == "enabled")
                .unwrap()
                .value,
            "false"
        );
        e.set("enabled", "false").unwrap();
        assert_eq!(e.document, before);
        assert!(e.set("enabled", "invalid").is_err());
        assert_eq!(e.document, before);
        e.set("enabled", "true").unwrap();
        let enabled = e.merged(&before).unwrap();
        assert!(enabled.ambient.routes[0].enabled);
        assert!(enabled.lighting.setup.devices[0].enabled);
        assert_eq!(enabled.ambient.sources, before.ambient.sources);
        assert_eq!(enabled.lcd, before.lcd);
        e.set("enabled", "false").unwrap();
        let disabled = e.merged(&before).unwrap();
        assert!(!disabled.ambient.routes[0].enabled);
        assert!(!disabled.lighting.setup.devices[0].enabled);
    }
    #[test]
    fn skydimo_role_change_rebuilds_only_its_map_and_disables_output() {
        let mut e = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        e.add_device_with_leds("SK0410", 3).unwrap();
        let id = e.document.lighting.setup.devices[e.index].id.clone();
        let before = e.document.clone();
        e.set("role", "canvas").unwrap();
        assert_eq!(e.document, before);
        e.set("role", "ambient").unwrap();
        assert!(
            e.document
                .lighting
                .setup
                .canvas
                .points
                .iter()
                .all(|p| p.device != id)
        );
        assert!(!e.document.lighting.setup.devices[e.index].enabled);
        let device_index = e.index;
        e.page = Page::Ambient;
        e.add_ambient_route(&id, Some("main")).unwrap();
        e.page = Page::Devices;
        e.index = device_index;
        e.set("role", "canvas").unwrap();
        assert_eq!(
            e.document
                .lighting
                .setup
                .canvas
                .points
                .iter()
                .filter(|p| p.device == id)
                .count(),
            3
        );
        assert!(!e.document.ambient.routes.iter().any(|r| r.device == id));
        assert_eq!(e.document.ambient.sources, before.ambient.sources);
        assert_eq!(e.document.lcd, before.lcd);
        assert_eq!(e.document.lighting.scene, before.lighting.scene);
        e.document.validate().unwrap();
    }
    #[test]
    fn ambient_creation_is_transactional_and_removal_preserves_shared_source() {
        let mut e = Editor {
            document: document(),
            page: Page::Ambient,
            index: 0,
        };
        e.document.lighting.setup.devices[0].driver = Driver::SkydimoSerial {
            model: "SK0410".into(),
        };
        e.document.ambient.routes.clear();
        assert_eq!(e.ambient_candidates()[0].0, "screen");
        let before = e.document.clone();
        for (device, source) in [("lcd", None), ("screen", Some("missing"))] {
            assert!(e.add_ambient_route(device, source).is_err());
            assert_eq!(e.document, before);
        }
        e.add_ambient_route("screen", Some("main")).unwrap();
        assert!(!e.document.ambient.routes[0].enabled);
        assert_eq!(e.document.ambient.routes[0].zone.leds, 2);
        assert_eq!(e.document.ambient.sources, before.ambient.sources);
        assert!(e.ambient_candidates().is_empty());
        let added = e.document.clone();
        assert!(e.add_ambient_route("screen", None).is_err());
        assert_eq!(e.document, added);
        e.document.ambient.routes[0].enabled = true;
        e.document.lighting.setup.devices[0].enabled = true;
        e.remove_ambient_route().unwrap();
        assert!(!e.document.lighting.setup.devices[0].enabled);
        assert!(e.document.ambient.routes.is_empty());
        assert_eq!(e.document.ambient.sources, before.ambient.sources);
        assert_eq!(e.document.lcd, before.lcd);
        e.document.validate().unwrap();
        e.add_ambient_route("screen", None).unwrap();
        assert_eq!(e.document.ambient.routes[0].source, "screen-1");
        assert_eq!(e.document.ambient.sources.len(), 2);
        assert_eq!(
            Document::decode(&e.document.encode().unwrap()).unwrap(),
            e.document
        );
    }
    #[test]
    fn ambient_source_limit_still_allows_an_existing_source() {
        let mut e = Editor {
            document: document(),
            page: Page::Ambient,
            index: 0,
        };
        e.document.ambient.routes.clear();
        while e.document.ambient.sources.len() < 16 {
            let mut source = e.document.ambient.sources[0].clone();
            source.key = format!("screen-{}", e.document.ambient.sources.len());
            e.document.ambient.sources.push(source);
        }
        let before = e.document.clone();
        assert!(e.add_ambient_route("screen", None).is_err());
        assert_eq!(e.document, before);
        e.add_ambient_route("screen", Some("main")).unwrap();
        assert_eq!(e.document.ambient.sources.len(), 16);
    }

    #[test]
    fn existing_home_assistant_route_can_return_to_scenes_without_offering_new_routes() {
        let mut legacy = document();
        let device = &mut legacy.lighting.setup.devices[0];
        device.driver = Driver::HomeAssistant;
        device.leds = 1;
        device.rate_hz = 1;
        device.enabled = true;
        legacy.ambient.routes[0].zone = Zone::linear(1, false, Mode::ScreenCopy).unwrap();
        let mut editor = Editor {
            document: Document::decode(&legacy.encode().unwrap()).unwrap(),
            page: Page::Ambient,
            index: 0,
        };
        assert_eq!(
            editor.ambient_route_notice(),
            Some(LEGACY_HOME_ASSISTANT_ROUTE)
        );
        assert_eq!(editor.document, legacy);
        editor.remove_ambient_route().unwrap();
        assert!(editor.ambient_route_notice().is_none());
        assert!(editor.ambient_candidates().is_empty());
        let without_route = editor.document.clone();
        assert!(editor.add_ambient_route("screen", Some("main")).is_err());
        assert_eq!(editor.document, without_route);

        editor.document = legacy.clone();
        editor.page = Page::Devices;
        let role = editor
            .fields()
            .into_iter()
            .find(|f| f.key == "role")
            .unwrap();
        assert_eq!(role.hint, LEGACY_HOME_ASSISTANT_ROUTE);
        assert!(role.choices.iter().any(|(_, value)| value == "canvas"));
        editor.set("role", "canvas").unwrap();
        assert!(editor.fields().iter().all(|field| field.key != "role"));
        assert!(editor.set("role", "ambient").is_err());
        editor.set("enabled", "true").unwrap();

        let saved = editor.merged(&legacy).unwrap().encode().unwrap();
        let reopened = Document::decode(&saved).unwrap();
        let device = &reopened.lighting.setup.devices[0];
        assert_eq!(device.id, "screen");
        assert_eq!(device.driver, Driver::HomeAssistant);
        assert_eq!(device.role, Role::Canvas);
        assert!(device.enabled);
        assert!(
            reopened
                .lighting
                .setup
                .canvas
                .points
                .iter()
                .any(|p| p.device == device.id)
        );
        assert!(reopened.ambient.routes.is_empty());
        assert_eq!(reopened.ambient.sources, legacy.ambient.sources);
        assert_eq!(reopened.lighting.scene, legacy.lighting.scene);
        assert_eq!(reopened.lighting.groups, legacy.lighting.groups);
    }
    #[test]
    fn adding_lcd_selects_its_configuration_not_its_setup_index() {
        let mut e = Editor {
            document: document(),
            page: Page::Lcd,
            index: 0,
        };
        e.add_lcd_device().unwrap();
        assert_eq!(e.document.lcd.len(), 2);
        assert_eq!(e.index, 1); // setup index is 2: one ambient precedes both LCDs.
        assert!(!e.document.lcd[e.index].config.enabled);
        assert!(e.document.lcd[e.index].config.media.is_none());
        assert!(e.fields().iter().any(|f| f.key == "media"));
        let new_id = &e.document.lcd[e.index].device;
        assert!(
            !e.document
                .lighting
                .setup
                .devices
                .iter()
                .find(|d| d.id == *new_id)
                .unwrap()
                .enabled
        );
    }
    #[test]
    fn every_ambient_property_roundtrips_without_losing_hidden_zone_or_current_scene() {
        let original = document();
        let mut editor = Editor {
            document: original.clone(),
            page: Page::Ambient,
            index: 0,
        };
        for field in editor.fields() {
            editor.set(field.key, &field.value).unwrap();
        }
        assert_eq!(editor.document, original);
        editor.set("gamma", "1.8").unwrap();
        editor.set("crop", "2, 3, 800, 600").unwrap();
        editor.set("brightness", "42.5").unwrap();
        let mut current = original;
        current.lighting.scene.brightness = 17;
        let next = editor.merged(&current).unwrap();
        assert_eq!(next.lighting.scene.brightness, 17);
        assert_eq!(next.ambient.routes[0].processing.correction.gamma, 1.8);
        assert_eq!(next.ambient.routes[0].zone, current.ambient.routes[0].zone);
        assert_eq!(next.ambient.sources[0].crop.unwrap().x, 2);
        assert_eq!(next.ambient.routes[0].output.brightness, 0.425);
    }
    #[test]
    fn invalid_drafts_never_become_an_applied_document() {
        let mut editor = Editor {
            document: document(),
            page: Page::Ambient,
            index: 0,
        };
        let original = editor.document.clone();
        assert!(editor.set("gamma", "not a number").is_err());
        assert_eq!(editor.document, original);
        editor.set("gamma", "NaN").unwrap();
        assert!(editor.merged(&original).is_err());
        assert!(editor.set("crop", "1,2,3").is_err());
    }
    #[test]
    fn logical_rename_updates_mean_and_lcd_references_without_changing_input_binding() {
        let mut editor = Editor {
            document: document(),
            page: Page::Temperatures,
            index: 0,
        };
        editor.set("kind", "sensor").unwrap();
        assert_eq!(
            editor.document.temperatures.sources[0].value,
            sensors::Value::Sensor {
                input: "slot-a".into()
            }
        );
        editor.set("key", "ram-a").unwrap();
        editor.document.validate().unwrap();
        assert!(
            matches!(&editor.document.temperatures.sources[2].value,sensors::Value::Mean{members}if members[0]=="ram-a")
        );
        assert!(editor.set("key", "b").is_err());
        editor.index = 2;
        editor.set("key", "memory").unwrap();
        assert_eq!(editor.document.lcd[0].config.sources[0].key, "memory");
    }
    #[test]
    fn selected_connection_uses_output_identity_for_each_editor_page() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 1,
        };
        assert_eq!(editor.selected_connection_device(), Some("lcd"));
        editor.index = 0;
        editor.page = Page::Lcd;
        assert_eq!(editor.selected_connection_device(), Some("lcd"));
        editor.page = Page::Ambient;
        assert_eq!(editor.selected_connection_device(), Some("screen"));
        editor.page = Page::Temperatures;
        assert_eq!(editor.selected_connection_device(), None);
    }

    #[test]
    fn output_targets_distinguish_same_names_in_route_order() {
        let mut editor = Editor {
            document: document(),
            page: Page::Lcd,
            index: 1,
        };
        let document = &mut editor.document;
        let devices = &mut document.lighting.setup.devices;
        for (index, id, label) in [
            (1, "lcd-second", "Помпа"),
            (1, "lcd-unique", "Дополнительный дисплей"),
            (0, "screen-second", "Экранная лента"),
            (0, "screen-unique", "Стена"),
        ] {
            let mut device = devices[index].clone();
            device.id = id.into();
            device.label = label.into();
            devices.push(device);
        }
        for id in ["lcd-unique", "lcd-second"] {
            let mut entry = document.lcd[0].clone();
            entry.device = id.into();
            document.lcd.insert(0, entry);
        }
        for id in ["screen-unique", "screen-second"] {
            let mut route = document.ambient.routes[0].clone();
            route.device = id.into();
            document.ambient.routes.insert(0, route);
        }

        assert_eq!(
            Editor::targets(document, Page::Lcd),
            [
                "Помпа (lcd-second)",
                "Дополнительный дисплей",
                "Помпа (lcd)"
            ]
        );
        assert_eq!(
            Editor::targets(document, Page::Ambient),
            [
                "Экранная лента (screen-second)",
                "Стена",
                "Экранная лента (screen)"
            ]
        );
        editor.index = 0;
        assert_eq!(editor.selected_connection_device(), Some("lcd-second"));
        editor.page = Page::Ambient;
        assert_eq!(editor.selected_connection_device(), Some("screen-second"));
    }

    #[test]
    fn lcd_selection_keeps_disabled_profile_valid_and_does_not_invent_an_asset() {
        let mut editor = Editor {
            document: document(),
            page: Page::Lcd,
            index: 0,
        };
        editor.set("rotation", "90").unwrap();
        editor.set("sources", "a=CPU; b=GPU; ram=RAM").unwrap();
        editor.document.validate().unwrap();
        editor.set("enabled", "true").unwrap();
        assert!(editor.document.lighting.setup.devices[1].enabled);
        assert!(editor.document.validate().is_err());
        assert!(editor.document.lcd[0].config.media.is_none());
        editor.set("enabled", "false").unwrap();
        editor.document.validate().unwrap();
    }

    #[test]
    fn lcd_source_hint_uses_profile_keys_and_multiple_assignments_round_trip() {
        let current = document();
        let mut editor = Editor {
            document: current.clone(),
            page: Page::Lcd,
            index: 0,
        };
        let source_hint = |editor: &Editor| {
            editor
                .fields()
                .into_iter()
                .find(|field| field.key == "sources")
                .unwrap()
                .hint
        };
        assert!(source_hint(&editor).contains("«a=A; b=B»"));
        assert!(!source_hint(&editor).contains("cpu=CPU"));

        editor.set("enabled", "true").unwrap();
        editor.set("overlay", "true").unwrap();
        editor.set("sources", "b=GPU; a=CPU").unwrap();
        let saved = editor.merged(&current).unwrap().encode().unwrap();
        let reopened = Document::decode(&saved).unwrap();
        let labels: Vec<_> = reopened.lcd[0]
            .config
            .sources
            .iter()
            .map(|source| (source.key.as_str(), source.label.as_str()))
            .collect();
        assert_eq!(labels, [("b", "GPU"), ("a", "CPU")]);

        editor.set("sources", "").unwrap();
        assert_eq!(
            editor.merged(&current).unwrap_err(),
            "Выберите хотя бы один показатель для панели температур."
        );
        editor.document.temperatures.sources.clear();
        assert_eq!(
            source_hint(&editor),
            "Сначала добавьте источник в разделе «Температуры»."
        );
    }

    #[test]
    fn lcd_source_choice_appends_once_and_preserves_manual_assignments() {
        let current = document();
        let mut editor = Editor {
            document: current.clone(),
            page: Page::Lcd,
            index: 0,
        };
        let choices = |editor: &Editor| {
            editor
                .fields()
                .into_iter()
                .find(|field| field.key == "add_source")
                .unwrap()
                .choices
        };
        assert_eq!(
            choices(&editor),
            [
                ("Выберите источник…".into(), "".into()),
                ("a".into(), "a".into()),
                ("b".into(), "b".into()),
            ]
        );
        editor.set("enabled", "true").unwrap();
        editor.set("overlay", "true").unwrap();
        editor.set("sources", "ram=Память; a=Процессор").unwrap();
        editor.set("add_source", "b").unwrap();
        editor.set("add_source", "b").unwrap();
        assert!(editor.set("add_source", "missing").is_err());
        assert!(editor.set("add_source", "").is_err());
        assert_eq!(
            choices(&editor),
            [("Все источники добавлены".into(), "".into())]
        );
        let reopened =
            Document::decode(&editor.merged(&current).unwrap().encode().unwrap()).unwrap();
        let assignments: Vec<_> = reopened.lcd[0]
            .config
            .sources
            .iter()
            .map(|source| (source.key.as_str(), source.label.as_str()))
            .collect();
        assert_eq!(
            assignments,
            [("ram", "Память"), ("a", "Процессор"), ("b", "B")]
        );
    }

    #[test]
    fn lcd_editor_clears_the_background_and_keeps_temperature_enabled() {
        let mut editor = Editor {
            document: document(),
            page: Page::Lcd,
            index: 0,
        };
        editor.document.lcd[0].config.media =
            Some(crate::app::assets::AssetId::try_from("a".repeat(64)).unwrap());
        editor.set("enabled", "true").unwrap();
        assert!(editor.set("media", "C:/file.png").is_err());
        editor.set("media", "").unwrap();
        assert!(editor.document.validate().is_err());
        editor.set("overlay", "true").unwrap();
        let reopened = Document::decode(&editor.document.encode().unwrap()).unwrap();
        let config = &reopened.lcd[0].config;
        assert!(config.enabled && config.overlay_enabled);
        assert_eq!(config.sources[0].key, "ram");
        assert!(reopened.assets().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn recreated_network_device_cannot_inherit_the_removed_devices_binding() {
        use crate::app::{lighting_bindings::Draft, lighting_outputs::Local};
        for (preset, draft) in [
            (
                "home-assistant-light",
                Draft {
                    endpoint: "http://127.0.0.1:8123".into(),
                    identity: "light.selected_room".into(),
                    serial: crate::devices::home_assistant::credential_id("http://127.0.0.1:8123")
                        .unwrap(),
                },
            ),
            (
                "wled-ddp",
                Draft {
                    endpoint: "127.0.0.1:4048".into(),
                    ..Default::default()
                },
            ),
        ] {
            let mut editor = Editor {
                document: document(),
                page: Page::Devices,
                index: 0,
            };
            editor.document.ambient = Default::default();
            editor.document.lighting.setup.canvas.width = 100;
            editor.add_device(preset).unwrap();
            let original = &editor.document.lighting.setup.devices[editor.index];
            let original_id = original.id.clone();
            let local = Local {
                version: 1,
                devices: [(original_id, draft.binding(original).unwrap())].into(),
                monitors: Default::default(),
                door_brightness: 15,
            };
            editor.set("enabled", "true").unwrap();
            let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            local.prepare(&editor.document, assets).unwrap();

            editor.remove_device().unwrap();
            let mut reopened = Editor {
                document: serde_json::from_slice(&serde_json::to_vec(&editor.document).unwrap())
                    .unwrap(),
                page: Page::Devices,
                index: 0,
            };
            reopened.add_device(preset).unwrap();
            reopened.set("enabled", "true").unwrap();
            assert!(
                local.prepare(&reopened.document, assets).is_err(),
                "{preset} inherited an obsolete local binding"
            );
            let replacement = &reopened.document.lighting.setup.devices[reopened.index];
            let reassigned = Local {
                devices: [(replacement.id.clone(), draft.binding(replacement).unwrap())].into(),
                ..local
            };
            reassigned.prepare(&reopened.document, assets).unwrap();
        }
    }

    #[test]
    fn device_commands_preserve_draft_and_geometry_and_rollback_invalid_edits() {
        let original = document();
        let mut editor = Editor {
            document: original.clone(),
            page: Page::Devices,
            index: 0,
        };
        editor.document.lighting.setup.canvas.width = 100;
        editor.document.lighting.setup.canvas.height = 100;
        editor.add_device("door20").unwrap();
        let id = editor.document.lighting.setup.devices[editor.index]
            .id
            .clone();
        editor.set("layout_line", "1,2,77,88").unwrap();
        editor.set("point", "3,99,0").unwrap();
        let points = editor.document.lighting.setup.canvas.points.clone();
        editor.set("name", "Дверь").unwrap();
        editor.set("rate", "48").unwrap();
        editor.set("group", "pc").unwrap();
        assert_eq!(editor.document.lighting.setup.canvas.points, points);
        let before = editor.document.clone();
        for (key, value) in [
            ("point", "3,100,0"),
            ("layout_grid", "0,0,100,100,0,true"),
            ("preset", "vk99"),
            ("rate", "0"),
        ] {
            assert!(editor.set(key, value).is_err());
            assert_eq!(editor.document, before);
        }
        let mut live = original.clone();
        live.lighting.scene.brightness = 13;
        let merged = editor.merged(&live).unwrap();
        assert_eq!(merged.lighting.scene.brightness, 13);
        assert_eq!(merged.lighting.setup, editor.document.lighting.setup);
        editor.set("replace_line", "vk99;0,0,99,99").unwrap();
        assert_eq!(editor.document.lighting.setup.devices[editor.index].id, id);
        assert_eq!(editor.document.lighting.setup.canvas.points.len(), 102);
        assert!(!editor.document.lighting.setup.devices[editor.index].enabled);
        editor.remove_device().unwrap();
        let mut expected = original;
        expected.lighting.setup.canvas.width = 100;
        expected.lighting.setup.canvas.height = 100;
        assert_eq!(editor.document, expected);
    }
    #[test]
    fn device_selection_follows_identity_after_draft_removal() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        editor.add_device("door20").unwrap();
        editor.index = 0;
        editor.remove_device().unwrap();
        assert!(editor.selected_device_id().is_none());
        assert!(editor.fields().is_empty());

        editor.select_device("lcd").unwrap();
        assert_eq!(editor.index, 0);
        assert_eq!(
            editor.document.lighting.setup.devices[editor.index].id,
            "lcd"
        );
        assert!(editor.select_device("screen").is_err());
        assert_eq!(editor.index, 0);

        editor.page = Page::Lcd;
        assert!(editor.select_device("lcd").is_err());
    }

    #[test]
    fn device_selection_survives_reordered_profile_and_clears_on_removal() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        for device in &mut editor.document.lighting.setup.devices {
            device.label = "Fixture".into();
        }
        editor.select_device("lcd").unwrap();
        let mut next = editor.document.clone();
        next.lighting.setup.devices.swap(0, 1);
        assert_eq!(editor.device_index_in(&next), 0);
        next.lighting
            .setup
            .devices
            .retain(|device| device.id != "lcd");
        editor.index = editor.device_index_in(&next);
        editor.document = next;
        assert!(editor.selected_device_id().is_none());
        assert!(editor.fields().is_empty());
    }

    #[test]
    fn output_editor_reports_logical_ids_for_draft_fields() {
        let mut editor = Editor {
            document: document(),
            page: Page::Lcd,
            index: 0,
        };
        assert_eq!(editor.selected_output_id(), Some("lcd"));
        editor.page = Page::Ambient;
        assert_eq!(editor.selected_output_id(), Some("screen"));
        editor.index = 1;
        assert_eq!(editor.selected_output_id(), None);
    }

    #[test]
    fn device_group_labels_match_scene_controls_and_keep_profile_values() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        let field = editor
            .device_fields()
            .into_iter()
            .find(|field| field.key == "group")
            .unwrap();
        assert_eq!(field.value, "pc");
        assert_eq!(
            field.choices,
            vec![
                ("Основной свет".into(), "pc".into()),
                ("Дополнительный свет".into(), "door".into()),
            ]
        );

        editor.set("group", "door").unwrap();
        assert_eq!(editor.document.lighting.groups.door, ["screen"]);
    }

    #[test]
    fn device_remove_cleans_references_and_lcd_add_invents_no_asset() {
        let original = document();
        let removed = device_setup::remove(&original, "screen").unwrap();
        assert!(removed.ambient.routes.is_empty());
        assert_eq!(removed.ambient.sources, original.ambient.sources);
        assert_eq!(removed.temperatures, original.temperatures);
        assert_eq!(removed.lcd, original.lcd);
        let removed = device_setup::remove(&original, "lcd").unwrap();
        assert!(removed.lcd.is_empty());
        let mut editor = Editor {
            document: original.clone(),
            page: Page::Devices,
            index: 0,
        };
        editor.add_device("tt-lcd").unwrap();
        let last = editor.document.lcd.last().unwrap();
        assert_eq!(last.config, lcd::Config::default());
        assert!(!editor.document.lighting.setup.devices[editor.index].enabled);
        let before = editor.document.clone();
        assert!(editor.set("enabled", "true").is_err());
        assert_eq!(editor.document, before);
        editor.remove_device().unwrap();
        assert_eq!(editor.document, original);
    }
    #[test]
    fn lcd_metadata_keeps_configured_playback_when_device_is_globally_disabled() {
        let mut original = document();
        original.lcd[0].config.enabled = true;
        original.lcd[0].config.media =
            Some(serde_json::from_value(serde_json::json!("a".repeat(64))).unwrap());
        original.validate().unwrap();
        let index = original
            .lighting
            .setup
            .devices
            .iter()
            .position(|d| d.id == "lcd")
            .unwrap();
        let mut editor = Editor {
            document: original.clone(),
            page: Page::Devices,
            index,
        };
        for (key, value) in [("name", "Новый LCD"), ("rate", "30"), ("group", "door")] {
            editor.set(key, value).unwrap();
            assert_eq!(editor.document.lcd, original.lcd);
            assert!(!editor.document.lighting.setup.devices[index].enabled);
        }
    }
    #[test]
    fn fusion_headers_and_explicit_grid_preserve_other_points() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        editor.document.lighting.setup.canvas.width = 11;
        editor.document.lighting.setup.canvas.height = 7;
        editor.add_device("fusion2-27-27").unwrap();
        let before = editor.document.clone();
        editor.set("headers", "32,22").unwrap();
        assert_eq!(
            editor.document.lighting.setup.canvas.points,
            before.lighting.setup.canvas.points
        );
        let before = editor.document.clone();
        assert!(editor.set("headers", "1,1").is_err());
        assert_eq!(editor.document, before);
        editor
            .replace_headers_with_layout(
                [1, 1],
                Layout::Grid {
                    x: 0,
                    y: 0,
                    width: 11,
                    height: 7,
                    columns: 2,
                    serpentine: true,
                },
            )
            .unwrap();
        let points = &editor.document.lighting.setup.canvas.points;
        assert_eq!((points[0].x, points[0].y), (0, 0));
        assert_eq!((points[1].x, points[1].y), (10, 0));
        let id = editor.document.lighting.setup.devices[editor.index]
            .id
            .clone();
        let mut d = editor.document.lighting.setup.devices[editor.index].clone();
        d.id = "changed".into();
        assert!(
            device_setup::replace(&editor.document, &id, d, crate::schedule::Target::Pc, None)
                .is_err()
        );
        let json = serde_json::to_string(&editor.document).unwrap();
        assert!(!json.contains("hid_path"));
        assert!(!json.contains("serial_port"));
        for p in device_setup::catalog() {
            if p.id == "SK0410" {
                assert!(p.device("test").is_err());
                p.device_with_leds("test", 1).unwrap();
                p.device_with_leds("test", 300).unwrap();
                assert!(p.device_with_leds("test", 301).is_err());
            } else {
                p.device("test").unwrap();
            }
        }
    }
    #[test]
    fn variable_counts_and_interleaved_point_order_are_explicit_and_bounded() {
        let mut editor = Editor {
            document: document(),
            page: Page::Devices,
            index: 0,
        };
        editor.document.lighting.setup.canvas.width = 11;
        editor.document.lighting.setup.canvas.height = 7;
        let before = editor.document.clone();
        assert!(editor.add_device("SK0410").is_err());
        assert_eq!(editor.document, before);
        editor.add_device_with_leds("SK0410", 3).unwrap();
        editor.set("leds_line", "4;0,0,10,6").unwrap();
        let first = editor.document.lighting.setup.devices[editor.index]
            .id
            .clone();
        editor.add_device("ene8").unwrap();
        // Deliberately non-device/non-LED ordering, as an imported map can have.
        editor.document.lighting.setup.canvas.points.reverse();
        let before = editor.document.clone();
        editor.set("name", "RAM").unwrap();
        assert_eq!(
            editor.document.lighting.setup.canvas.points,
            before.lighting.setup.canvas.points
        );
        editor.set("layout_grid", "0,0,11,7,3,true").unwrap();
        for (old, new) in before
            .lighting
            .setup
            .canvas
            .points
            .iter()
            .zip(&editor.document.lighting.setup.canvas.points)
        {
            assert_eq!((&old.device, old.led), (&new.device, new.led));
            if old.device == first {
                assert_eq!(old, new);
            }
        }
        let before = editor.document.clone();
        assert!(editor.set("leds_line", "7;0,0,10,6").is_err());
        assert_eq!(editor.document, before);
        editor.index = 2;
        let before = editor.document.clone();
        assert!(editor.set("leds_line", "301;0,0,10,6").is_err());
        assert_eq!(editor.document, before);
        editor.index = 0;
        editor.replace_preset("tt-lcd", None).unwrap();
        assert!(editor.document.ambient.routes.is_empty());
        assert!(
            editor
                .document
                .lcd
                .iter()
                .any(|r| r.device == "screen" && r.config == lcd::Config::default())
        );
        assert_eq!(editor.document.temperatures, before.temperatures);
    }
}
