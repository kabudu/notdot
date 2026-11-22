//! A private browser that notdot launches and drives itself.
//!
//! Every launch gets a new, empty profile in a temporary directory that is
//! deleted when the browser closes, so the person's own browser profile,
//! cookies, passwords and private windows are never reachable. Control uses
//! the DevTools pipe, never a debugging port. Page scripts cannot see notdot's
//! element references: they live in a separate isolated JavaScript world.
#![cfg_attr(not(any(unix, windows)), allow(dead_code, unused_imports))]
use crate::{Status, clean};
use serde::Serialize;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

const STEP: Duration = Duration::from_secs(10);
const LOAD: Duration = Duration::from_secs(15);
/// Largest page text returned to the model, in characters.
pub const MAX_TEXT: usize = 6000;
pub const MAX_ELEMENTS: usize = 150;

/// Chromium-family browsers notdot can launch with a disposable profile.
fn candidates() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("NOTDOT_HOST_BROWSER") {
        return vec![PathBuf::from(path)];
    }
    #[cfg(target_os = "macos")]
    {
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let names = [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "brave-browser",
        ];
        std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .flat_map(|root| {
                [
                    "Google/Chrome/Application/chrome.exe",
                    "Chromium/Application/chrome.exe",
                    "Microsoft/Edge/Application/msedge.exe",
                    "BraveSoftware/Brave-Browser/Application/brave.exe",
                ]
                .into_iter()
                .map(move |name| PathBuf::from(&root).join(name))
            })
            .collect()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Vec::new()
    }
}

pub fn executable() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

pub fn readiness() -> Status {
    if !cfg!(any(unix, windows)) {
        return Status::Unsupported(
            "notdot's private browser is unavailable on this system.".into(),
        );
    }
    match executable() {
        Some(_) => Status::Ready,
        None => Status::SetupRequired(
            "Install Google Chrome, Chromium, Microsoft Edge or Brave. notdot opens it with its own empty profile, never yours.".into(),
        ),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Element {
    #[serde(rename = "ref")]
    pub reference: u32,
    pub role: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    pub elements: Vec<Element>,
    pub text: String,
    /// Changes with every snapshot; element refs are valid only for it.
    pub snapshot: u64,
}

/// What a reference points at, checked before any confirmation or action.
#[derive(Clone, Debug)]
pub struct Target {
    pub role: String,
    pub label: String,
    pub input_type: Option<String>,
    /// Password, payment card and one-time-code fields: never typed into.
    pub secret: bool,
    pub editable: bool,
}

/// A page target in the disposable browser.
pub struct Tab {
    pub id: String,
    pub url: String,
    pub current: bool,
}

#[cfg(any(unix, windows))]
pub struct Browser {
    cdp: crate::cdp::Cdp,
    session: String,
    target: String,
    world: Option<(u64, i64)>,
    generation: u64,
}

#[cfg(any(unix, windows))]
impl Browser {
    pub async fn launch(headless: bool) -> Result<Self, String> {
        let executable = executable().ok_or("No supported browser is installed")?;
        let cdp = crate::cdp::Cdp::launch(&executable, headless)?;
        let _ = cdp
            .call(
                None,
                "Browser.setDownloadBehavior",
                json!({"behavior":"deny"}),
                STEP,
            )
            .await;
        let targets = cdp.call(None, "Target.getTargets", json!({}), STEP).await?;
        let page = targets["targetInfos"]
            .as_array()
            .and_then(|t| t.iter().find(|t| t["type"] == "page"))
            .and_then(|t| t["targetId"].as_str().map(str::to_owned));
        let target = match page {
            Some(id) => id,
            None => cdp
                .call(
                    None,
                    "Target.createTarget",
                    json!({"url":"about:blank"}),
                    STEP,
                )
                .await?["targetId"]
                .as_str()
                .ok_or("Browser did not create a page")?
                .to_owned(),
        };
        let session = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                STEP,
            )
            .await?["sessionId"]
            .as_str()
            .ok_or("Browser did not attach to the page")?
            .to_owned();
        cdp.call(Some(&session), "Page.enable", json!({}), STEP)
            .await?;
        Ok(Self {
            cdp,
            session,
            target,
            world: None,
            generation: 0,
        })
    }

    pub fn alive(&self) -> bool {
        self.cdp.alive()
    }

    /// The browser's process id, for diagnostics and tests.
    pub fn process_id(&self) -> Option<u32> {
        self.cdp.process_id()
    }

    async fn page(&self, method: &str, params: Value) -> Result<Value, String> {
        if !self.cdp.alive() {
            return Err("notdot's private browser closed".into());
        }
        self.cdp
            .call(Some(&self.session), method, params, STEP)
            .await
    }

    pub async fn tabs(&self) -> Result<Vec<Tab>, String> {
        let result = self
            .cdp
            .call(None, "Target.getTargets", json!({}), STEP)
            .await?;
        let targets = result["targetInfos"]
            .as_array()
            .ok_or("Browser did not report its tabs")?;
        Ok(targets
            .iter()
            .filter(|tab| tab["type"] == "page")
            .take(32)
            .filter_map(|tab| {
                let id = tab["targetId"].as_str()?;
                Some(Tab {
                    id: id.to_owned(),
                    url: tab["url"].as_str()?.to_owned(),
                    current: id == self.target,
                })
            })
            .collect())
    }

    pub async fn switch_tab(&mut self, target: &str) -> Result<(), String> {
        if target == self.target {
            return Ok(());
        }
        let session = self
            .cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                STEP,
            )
            .await?["sessionId"]
            .as_str()
            .ok_or("Browser did not attach to the tab")?
            .to_owned();
        let previous = std::mem::replace(&mut self.session, session);
        self.target = target.to_owned();
        self.world = None;
        let _ = self
            .cdp
            .call(
                None,
                "Target.detachFromTarget",
                json!({"sessionId":previous}),
                STEP,
            )
            .await;
        Ok(())
    }

    /// Capture only the visible viewport of the currently selected private tab.
    pub async fn screenshot(&self) -> Result<String, String> {
        let layout = self.page("Page.getLayoutMetrics", json!({})).await?;
        let viewport = &layout["cssVisualViewport"];
        let width = viewport["clientWidth"]
            .as_f64()
            .filter(|v| *v >= 1.0)
            .ok_or("No visible viewport")?;
        let height = viewport["clientHeight"]
            .as_f64()
            .filter(|v| *v >= 1.0)
            .ok_or("No visible viewport")?;
        let x = viewport["pageX"].as_f64().ok_or("No visible viewport")?;
        let y = viewport["pageY"].as_f64().ok_or("No visible viewport")?;
        let scale = (1280.0 / width).min(900.0 / height).min(1.0);
        let result = self
            .page(
                "Page.captureScreenshot",
                json!({
                    "format":"jpeg", "quality":65, "captureBeyondViewport":false,
                    "clip":{"x":x,"y":y,"width":width,"height":height,"scale":scale}
                }),
            )
            .await?;
        let data = result["data"]
            .as_str()
            .filter(|data| !data.is_empty() && data.len() <= 1_500_000)
            .ok_or("The screenshot could not be captured within the size limit")?;
        Ok(data.to_owned())
    }

    /// The page's current address, read from the browser, not the page.
    pub async fn url(&self) -> Result<String, String> {
        let history = self.page("Page.getNavigationHistory", json!({})).await?;
        let index = history["currentIndex"].as_u64().unwrap_or(0) as usize;
        Ok(history["entries"][index]["url"]
            .as_str()
            .unwrap_or("")
            .to_owned())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<(), String> {
        self.world = None;
        let result = self.page("Page.navigate", json!({"url":url})).await?;
        if let Some(error) = result["errorText"].as_str()
            && !error.is_empty()
        {
            return Err(format!(
                "The page could not be opened ({})",
                clean(error, 80)
            ));
        }
        self.settle().await
    }

    /// Wait for the document to finish loading, bounded.
    async fn settle(&mut self) -> Result<(), String> {
        let started = std::time::Instant::now();
        loop {
            let state = self
                .page(
                    "Runtime.evaluate",
                    json!({"expression":"document.readyState","returnByValue":true}),
                )
                .await
                .ok()
                .and_then(|r| r["result"]["value"].as_str().map(str::to_owned));
            if state.as_deref() == Some("complete") {
                return Ok(());
            }
            if !self.cdp.alive() {
                return Err("notdot's private browser closed".into());
            }
            if started.elapsed() > LOAD {
                return Err("The page took too long to load".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A fresh isolated world for this snapshot. Page scripts share the DOM
    /// but not JavaScript objects with it, so they cannot read or forge refs.
    async fn new_world(&mut self) -> Result<i64, String> {
        let tree = self.page("Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or("The page has no main frame")?;
        let world = self
            .page(
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"notdot","grantUniveralAccess":false}),
            )
            .await?;
        world["executionContextId"]
            .as_i64()
            .ok_or_else(|| "The page did not create notdot's isolated world".into())
    }

    async fn evaluate(&self, context: i64, expression: &str) -> Result<Value, String> {
        let result = self
            .page(
                "Runtime.evaluate",
                json!({"expression":expression,"contextId":context,"returnByValue":true,"awaitPromise":true}),
            )
            .await
            .map_err(|e| {
                if e.contains("context") {
                    "The page changed. Look at it again before acting.".to_owned()
                } else {
                    e
                }
            })?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(format!(
                "Page check failed: {}",
                clean(
                    exception["exception"]["description"]
                        .as_str()
                        .or(exception["text"].as_str())
                        .unwrap_or("error"),
                    160
                )
            ));
        }
        Ok(result["result"]["value"].clone())
    }

    pub async fn snapshot(&mut self) -> Result<Snapshot, String> {
        self.settle().await?;
        let context = self.new_world().await?;
        self.generation += 1;
        let raw = self
            .evaluate(
                context,
                &SNAPSHOT_SCRIPT.replace("__MAX__", &MAX_ELEMENTS.to_string()),
            )
            .await?;
        self.world = Some((self.generation, context));
        let elements = raw["elements"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .take(MAX_ELEMENTS)
                    .map(|e| Element {
                        reference: e["ref"].as_u64().unwrap_or(0) as u32,
                        role: clean(e["role"].as_str().unwrap_or(""), 24),
                        label: clean(e["label"].as_str().unwrap_or(""), 120),
                        input_type: e["type"].as_str().map(|t| clean(t, 24)),
                        disabled: e["disabled"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let text: String = raw["text"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(MAX_TEXT)
            .collect();
        Ok(Snapshot {
            url: raw["url"]
                .as_str()
                .ok_or("The page address is unavailable")?
                .to_owned(),
            title: clean(raw["title"].as_str().unwrap_or(""), 200),
            elements,
            text,
            snapshot: self.generation,
        })
    }

    fn world(&self, snapshot: u64) -> Result<i64, String> {
        match self.world {
            Some((generation, context)) if generation == snapshot => Ok(context),
            _ => Err("That element list is out of date. Look at the page again first.".into()),
        }
    }

    /// Describe a reference without acting, so policy can decide first.
    pub async fn target(&self, snapshot: u64, reference: u32) -> Result<Target, String> {
        let context = self.world(snapshot)?;
        let raw = self
            .evaluate(
                context,
                &format!("(() => {{ {LOOKUP} return describe(el); }})()")
                    .replace("__REF__", &reference.to_string()),
            )
            .await?;
        Ok(Target {
            role: clean(raw["role"].as_str().unwrap_or(""), 24),
            label: clean(raw["label"].as_str().unwrap_or(""), 120),
            input_type: raw["type"].as_str().map(|t| clean(t, 24)),
            secret: raw["secret"].as_bool().unwrap_or(true),
            editable: raw["editable"].as_bool().unwrap_or(false),
        })
    }

    /// Click the element's centre with a real (trusted) mouse event, after
    /// checking that it is still the described element and is not covered.
    pub async fn click(
        &mut self,
        snapshot: u64,
        reference: u32,
        label: &str,
    ) -> Result<(), String> {
        let context = self.world(snapshot)?;
        let point = self
            .evaluate(
                context,
                &format!("(() => {{ {LOOKUP} {POINT} }})()",)
                    .replace("__REF__", &reference.to_string()),
            )
            .await?;
        same_label(&point, label)?;
        let (Some(x), Some(y)) = (point["x"].as_f64(), point["y"].as_f64()) else {
            return Err("The element could not be located".into());
        };
        for (kind, count) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 1)] {
            self.page(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":x,"y":y,"button":"left","clickCount":count}),
            )
            .await?;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.settle().await
    }

    /// Focus an editable element and insert text as if typed.
    pub async fn type_text(
        &mut self,
        snapshot: u64,
        reference: u32,
        label: &str,
        text: &str,
    ) -> Result<(), String> {
        let context = self.world(snapshot)?;
        let ok = self
            .evaluate(
                context,
                &format!("(() => {{ {LOOKUP} {FOCUS} }})()")
                    .replace("__REF__", &reference.to_string()),
            )
            .await?;
        same_label(&ok, label)?;
        if ok["ok"] != true {
            return Err("That field can no longer be typed into".into());
        }
        self.page("Input.insertText", json!({"text":text})).await?;
        Ok(())
    }
}

/// The element must still carry the label the person confirmed.
fn same_label(found: &Value, expected: &str) -> Result<(), String> {
    if clean(found["label"].as_str().unwrap_or(""), 120) == expected {
        Ok(())
    } else {
        Err("That element changed. Look at the page again.".into())
    }
}

/// Collect visible interactive elements and page text in the isolated world.
const SNAPSHOT_SCRIPT: &str = r#"(() => {
  const refs = [];
  globalThis.__notdotRefs = refs;
  const visible = el => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
  };
  const label = el => {
    const byId = el.id && document.querySelector('label[for="' + CSS.escape(el.id) + '"]');
    const text = el.getAttribute('aria-label') || (byId && byId.innerText) ||
      (el.tagName === 'INPUT' ? '' : el.innerText) || el.getAttribute('placeholder') ||
      el.getAttribute('title') || el.getAttribute('name') || '';
    return String(text).replace(/\s+/g, ' ').trim().slice(0, 160);
  };
  const out = [];
  const selector = 'a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=switch],[role=tab],[role=menuitem],[contenteditable=""],[contenteditable=true]';
  for (const el of document.querySelectorAll(selector)) {
    if (out.length >= __MAX__) break;
    if (!visible(el)) continue;
    refs.push(el);
    out.push({ref: refs.length, role: el.getAttribute('role') || el.tagName.toLowerCase(),
      type: el.tagName === 'INPUT' ? (el.type || 'text') : null, label: label(el),
      disabled: !!el.disabled || el.getAttribute('aria-disabled') === 'true'});
  }
  const text = document.body ? document.body.innerText.replace(/\n{3,}/g, '\n\n') : '';
  return {url: location.href, title: document.title, elements: out, text: text.slice(0, 20000)};
})()"#;

/// Shared lookup: `el` is the referenced element or the script throws.
const LOOKUP: &str = r#"
  const refs = globalThis.__notdotRefs || [];
  const el = refs[__REF__ - 1];
  if (!el || !el.isConnected) throw new Error('That element is gone. Look at the page again.');
  const describe = el => {
    const type = el.tagName === 'INPUT' ? (el.type || 'text').toLowerCase() : null;
    const auto = (el.getAttribute('autocomplete') || '').toLowerCase();
    const secret = type === 'password' || auto.includes('password') || auto.startsWith('cc-') || auto === 'one-time-code';
    const editable = (el.tagName === 'INPUT' && !['button','submit','reset','checkbox','radio','file','image','range','color'].includes(type)) || el.tagName === 'TEXTAREA' || el.isContentEditable;
    const byId = el.id && document.querySelector('label[for="' + CSS.escape(el.id) + '"]');
    const label = String(el.getAttribute('aria-label') || (byId && byId.innerText) || (el.tagName === 'INPUT' ? '' : el.innerText) || el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('name') || '').replace(/\s+/g, ' ').trim().slice(0, 160);
    const submits = el.tagName === 'BUTTON' ? (el.type || 'submit') === 'submit' && !!el.form : type === 'submit';
    return {role: el.getAttribute('role') || el.tagName.toLowerCase(), label, type: submits ? 'submit' : type, secret, editable};
  };
"#;

const POINT: &str = r#"
  const now = describe(el);
  el.scrollIntoView({block: 'center', inline: 'center'});
  const r = el.getBoundingClientRect();
  const x = r.left + r.width / 2, y = r.top + r.height / 2;
  const hit = document.elementFromPoint(x, y);
  if (!hit || !(hit === el || el.contains(hit))) throw new Error('Something on the page covers that element, so it was not clicked.');
  return {x, y, label: now.label};
"#;

const FOCUS: &str = r#"
  const now = describe(el);
  if (!now.editable || now.secret) return {ok: false, label: now.label};
  el.scrollIntoView({block: 'center'});
  el.focus();
  return {ok: document.activeElement === el || el.contains(document.activeElement), label: now.label};
"#;

/// Windows has no private-pipe launch yet; every call reports that plainly.
#[cfg(not(any(unix, windows)))]
pub struct Browser;

#[cfg(not(any(unix, windows)))]
impl Browser {
    const UNSUPPORTED: &str = "notdot's private browser is unavailable on this system.";
    pub async fn screenshot(&self) -> Result<String, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn tabs(&self) -> Result<Vec<Tab>, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn switch_tab(&mut self, _target: &str) -> Result<(), String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn launch(_headless: bool) -> Result<Self, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub fn alive(&self) -> bool {
        false
    }
    pub fn process_id(&self) -> Option<u32> {
        None
    }
    pub async fn url(&self) -> Result<String, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn navigate(&mut self, _url: &str) -> Result<(), String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn snapshot(&mut self) -> Result<Snapshot, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn target(&self, _snapshot: u64, _reference: u32) -> Result<Target, String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn click(
        &mut self,
        _snapshot: u64,
        _reference: u32,
        _label: &str,
    ) -> Result<(), String> {
        Err(Self::UNSUPPORTED.into())
    }
    pub async fn type_text(
        &mut self,
        _snapshot: u64,
        _reference: u32,
        _label: &str,
        _text: &str,
    ) -> Result<(), String> {
        Err(Self::UNSUPPORTED.into())
    }
}
