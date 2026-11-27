// UI de Chispa: lee el estado del motor una vez por segundo (solo si la ventana está visible)
// y manda acciones. Toda la lógica vive en Rust.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s) => document.querySelector(s);
const $$ = (s) => document.querySelectorAll(s);
let cfg = null;
let st = null;
let lang = "es";
let toastTimer = null;
let busy = false;
let tab = "control";
let unread = 0;
const selTargets = new Set();
const PLAT = { twitch: "Twitch", youtube: "YouTube", kick: "Kick" };
const t = (k, ...a) => {
  const v = (I18N[lang] ?? I18N.es)[k] ?? I18N.es[k] ?? k;
  return typeof v === "function" ? v(...a) : v;
};

function applyI18n() {
  document.documentElement.lang = lang;
  $$("[data-i18n]").forEach((el) => (el.textContent = t(el.dataset.i18n)));
  $$("[data-lang]").forEach((b) => b.classList.toggle("sel", b.dataset.lang === lang));
}

function fmtTime(s) {
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  const mm = String(m).padStart(2, "0"), ss = String(x).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function toast(msg, ok = true) {
  const el = $("#toast");
  el.textContent = msg;
  el.className = "toast " + (ok ? "ok" : "err");
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), ok ? 2800 : 6000);
}

function pill(el, text, ok) {
  el.textContent = text;
  el.className = "pill " + (ok ? "ok" : "off");
}

function render(s) {
  st = s;
  if (s.error) {
    $("#boot").classList.add("error");
    $("#boot-msg").textContent = s.error;
    return;
  }
  if (!s.ready) return;
  $("#boot").hidden = true;
  $("#main").hidden = false;

  $("#chip").textContent = `${s.tier} · ${s.resolution} · ${s.fps} fps · ${s.encoder}`;
  $("#chip").title = `${s.threads} hilos, ${s.ram_gb} GB · ${s.video_kbps} kbps`;

  $("#btn-clip").disabled = !s.replay;
  $("#sub-clip").textContent = s.replay ? t("last_s", s.replay_seconds) : t("enable_clips");
  $("#row-vertical").disabled = !s.replay;

  $("#btn-record").classList.toggle("on", s.recording);
  $("#lbl-record").textContent = s.recording ? t("stop") : t("record");
  $("#sub-record").textContent = s.recording ? fmtTime(s.rec_seconds) : t("to_mp4");

  $("#btn-stream").classList.toggle("on", s.streaming);
  $("#lbl-stream").textContent = s.streaming ? t("stop_live") : t("live");
  if (s.streaming) {
    $("#sub-stream").textContent = `${s.stream_target} · ${fmtTime(s.stream_seconds)}`;
  } else if (cfg) {
    const names = cfg.targets.map((p) => PLAT[p]).join(" + ");
    const missing = cfg.targets.some((p) => !cfg.keys[p]);
    $("#sub-stream").textContent = !names ? t("pick_dest") : names + (missing ? ` · ${t("missing_key")}` : "");
  }

  // Guardián: salud de cada destino
  const bad = (s.stream_health ?? []).filter((h) => h.reconnecting || h.drop_pct >= 1);
  $("#health").hidden = !s.streaming || bad.length === 0;
  $("#health").textContent = bad
    .map((h) => (h.reconnecting ? t("reconnecting", h.name) : t("drops", h.name, h.drop_pct.toFixed(1))))
    .join(" · ");

  $$("[data-scene]").forEach((b) => b.classList.toggle("sel", b.dataset.scene === s.scene));

  $("#row-mic").disabled = !s.has_mic;
  pill($("#pill-mic"), !s.has_mic ? t("none") : s.mic_muted ? t("muted") : t("on"), s.has_mic && !s.mic_muted);
  $("#row-mic").title = s.mic_name ?? "";
  pill($("#pill-camera"), s.camera_on ? t("on") : t("off"), s.camera_on);
  $("#row-camera").title = s.camera_name ?? "";
  pill($("#pill-replay"), s.replay ? `${t("on")} · ${s.replay_seconds} s` : t("off"), s.replay);
  if (cfg) pill($("#pill-voice"), cfg.voice_on ? t("listening") : t("off"), cfg.voice_on);

  if (s.notice) toast(s.notice, false);
  schedulePreview();
}

async function tick() {
  if (document.hidden) return;
  try {
    render(await invoke("status"));
  } catch (e) {
    console.error(e);
  }
}

async function doAction(name, btn) {
  if (busy) return;
  busy = true;
  try {
    const msg = await invoke("action", { name });
    toast(msg, true);
    if (btn) {
      btn.classList.remove("flash");
      void btn.offsetWidth;
      btn.classList.add("flash");
    }
    if (name === "voice") await loadConfig();
  } catch (e) {
    toast(String(e), false);
    if (String(e).includes("Ajustes")) showTab("settings");
  } finally {
    busy = false;
    tick();
  }
}

// ---------- Vista previa (la dibuja libobs por GPU en una ventana nativa encima de #pv) ----------

let pvOn = localStorage.getItem("preview") !== "off";
let pvSent = null;

async function syncPreview() {
  const want = pvOn && tab === "control" && !document.hidden && st?.ready;
  $("#pv-toggle").textContent = pvOn ? t("preview_hide") : t("preview_show");
  $(".pv-wrap").classList.toggle("off", !pvOn);
  $("#pv-msg").hidden = want;
  if (!want) {
    if (pvSent !== null) {
      pvSent = null;
      await invoke("preview", { rect: null }).catch(() => {});
    }
    return;
  }
  const r = $("#pv").getBoundingClientRect();
  const k = window.devicePixelRatio || 1;
  const rect = { x: r.left * k, y: r.top * k, w: r.width * k, h: r.height * k };
  const key = JSON.stringify(rect);
  if (key === pvSent) return;
  pvSent = key;
  await invoke("preview", { rect }).catch((e) => {
    pvSent = null;
    pvOn = false;
    toast(String(e), false);
  });
}

let pvFrame = 0;
function schedulePreview() {
  cancelAnimationFrame(pvFrame);
  pvFrame = requestAnimationFrame(() => syncPreview());
}

function showTab(name) {
  tab = name;
  $$("[data-tab]").forEach((b) => b.classList.toggle("sel", b.dataset.tab === name));
  $$(".tab").forEach((s) => (s.hidden = s.id !== `tab-${name}`));
  schedulePreview();
  if (name === "chat") {
    unread = 0;
    $("#chat-badge").hidden = true;
    const list = $("#chat-list");
    list.scrollTop = list.scrollHeight;
  }
}

// ---------- Ajustes ----------

function renderTargets() {
  $$("[data-target]").forEach((b) => b.classList.toggle("sel", selTargets.has(b.dataset.target)));
}

async function loadConfig() {
  cfg = await invoke("get_config");
  lang = cfg.lang || "es";
  applyI18n();
  schedulePreview();
  for (const [action, key] of cfg.hotkeys) {
    $$(`kbd[data-hk="${action}"]`).forEach((k) => (k.textContent = key));
  }
  if (cfg.hotkeys_failed.length) {
    $("#hk-warn").hidden = false;
    $("#hk-warn").textContent = t("hk_taken", cfg.hotkeys_failed.join(", "));
  }
  selTargets.clear();
  cfg.targets.forEach((p) => selTargets.add(p));
  renderTargets();
  for (const p of Object.keys(PLAT)) {
    const saved = cfg.keys[p];
    $(`[data-keystate="${p}"]`).textContent = saved ? t("key_saved") : t("key_missing");
    $(`[data-key="${p}"]`).placeholder = saved ? t("key_keep") : t("key_paste");
    $(`[data-keyok="${p}"]`).textContent = saved ? "✓" : "";
  }
  $("#kick-url").value = cfg.kick_url;
  $("#tw-client").value = cfg.twitch_client_id;
  $("#tw-channel").value = cfg.twitch_channel;
  $("#kick-channel").value = cfg.kick_channel;
  $("#opt-chat").checked = cfg.chat;
  $("#opt-alerts").checked = cfg.alerts_on_stream;
  $("#tw-state").textContent = cfg.twitch_login ? t("connected_as", cfg.twitch_login) : t("not_connected");
  $("#tw-disconnect").hidden = !cfg.twitch_login;
  $("#tw-channel-wrap").hidden = !cfg.twitch_login;
  $("#chat-empty").hidden = !!(cfg.twitch_channel || cfg.kick_channel);
  if (st) render(st);
}

async function saveSettings() {
  const keys = {};
  $$("[data-key]").forEach((i) => {
    if (i.value) keys[i.dataset.key] = i.value;
  });
  const msg = await invoke("save_config", {
    settings: {
      targets: [...selTargets],
      kick_url: $("#kick-url").value,
      twitch_client_id: $("#tw-client").value,
      twitch_channel: $("#tw-channel").value,
      kick_channel: $("#kick-channel").value,
      lang,
      chat: $("#opt-chat").checked,
      alerts_on_stream: $("#opt-alerts").checked,
      keys,
    },
  });
  $$("[data-key]").forEach((i) => (i.value = ""));
  await loadConfig();
  return msg;
}

// ---------- Chat ----------

function addChat(ev) {
  const list = $("#chat-list");
  const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
  const row = document.createElement("div");
  row.className = `msg ${ev.kind} ${ev.platform}`;
  const tag = document.createElement("span");
  tag.className = "plat";
  tag.textContent = ev.platform === "kick" ? "K" : "T";
  row.append(tag);
  if (ev.user && ev.kind === "msg") {
    const u = document.createElement("b");
    u.textContent = ev.user;
    if (ev.color) u.style.color = ev.color;
    row.append(u, document.createTextNode(" "));
  }
  row.append(document.createTextNode(ev.text));
  list.append(row);
  while (list.children.length > 200) list.firstChild.remove();
  if (atBottom) list.scrollTop = list.scrollHeight;
  $("#chat-empty").hidden = true;
  if (tab !== "chat" && ev.kind !== "info") {
    unread++;
    $("#chat-badge").hidden = false;
    $("#chat-badge").textContent = unread > 99 ? "99+" : unread;
  }
}

// ---------- Eventos ----------

$$("[data-action]").forEach((b) => b.addEventListener("click", () => doAction(b.dataset.action, b)));
$$("[data-scene]").forEach((b) => b.addEventListener("click", () => doAction(`scene:${b.dataset.scene}`, b)));
$$("[data-tab]").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab)));
$$("[data-target]").forEach((b) =>
  b.addEventListener("click", () => {
    const p = b.dataset.target;
    selTargets.has(p) ? selTargets.delete(p) : selTargets.add(p);
    renderTargets();
  })
);
$$("[data-lang]").forEach((b) =>
  b.addEventListener("click", () => {
    lang = b.dataset.lang;
    applyI18n();
    if (st) render(st);
  })
);
$$("[data-folder]").forEach((b) =>
  b.addEventListener("click", () => invoke("open_folder", { which: b.dataset.folder }).catch((e) => toast(String(e), false)))
);
$$("[data-link]").forEach((b) => b.addEventListener("click", () => invoke("open_link", { url: b.dataset.link })));

$("#save").addEventListener("click", async () => {
  try {
    toast(await saveSettings(), true);
  } catch (e) {
    toast(String(e), false);
  }
});

$("#tw-connect").addEventListener("click", async () => {
  try {
    await saveSettings(); // guarda el Client ID antes de conectar
    const r = await invoke("twitch_connect");
    $("#tw-code").hidden = false;
    $("#tw-code").textContent = t("tw_code", r.user_code);
  } catch (e) {
    toast(String(e), false);
  }
});
$("#tw-disconnect").addEventListener("click", async () => {
  try {
    toast(await invoke("twitch_disconnect"), true);
    await loadConfig();
  } catch (e) {
    toast(String(e), false);
  }
});
$("#tw-update").addEventListener("click", async () => {
  try {
    toast(await invoke("twitch_update", { title: $("#tw-title").value, category: $("#tw-cat").value }), true);
  } catch (e) {
    toast(String(e), false);
  }
});

$("#stream-mode").addEventListener("click", async () => {
  pvSent = null;
  await invoke("preview", { rect: null }).catch(() => {});
  invoke("stream_mode");
});
$("#pv-toggle").addEventListener("click", () => {
  pvOn = !pvOn;
  localStorage.setItem("preview", pvOn ? "on" : "off");
  syncPreview();
});
document.body.addEventListener("scroll", schedulePreview);
window.addEventListener("resize", schedulePreview);

listen("toast", (e) => toast(e.payload.msg, e.payload.ok));
listen("chat", (e) => addChat(e.payload));
listen("twitch", () => {
  $("#tw-code").hidden = true;
  loadConfig();
});
document.addEventListener("visibilitychange", () => {
  tick();
  schedulePreview();
});

loadConfig()
  .then(() => invoke("chat_history"))
  .then((h) => h.forEach(addChat))
  .catch((e) => toast(String(e), false));
tick();
setInterval(tick, 1000);
