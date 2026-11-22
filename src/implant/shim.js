// SPDX-License-Identifier: GPL-3.0-only
// Comfy-GNOME "implant": runs custom-node frontend extensions unchanged, without the
// ComfyUI web UI. Extensions import `/scripts/app.js` etc. from the backend; those files
// are re-export shims over `window.comfyAPI.<module>.<name>`, so defining that object is
// the whole seam. Nothing is drawn: what extensions register (settings, node hooks) is
// reported to the GTK side, which renders it natively.
(() => {
  "use strict";
  const post = (m) => window.webkit.messageHandlers.cg.postMessage(JSON.stringify(m));
  const log = (lvl, a) => post({ t: "log", lvl, msg: a.map((x) => (x instanceof Error ? `${x}\n${x.stack}` : typeof x === "object" ? safeJson(x) : String(x))).join(" ") });
  const safeJson = (x) => { try { return JSON.stringify(x); } catch { return String(x); } };
  // The backend's own shim modules announce themselves on import; not news here.
  for (const lvl of ["error", "warn"]) console[lvl] = (...a) => /^\[ComfyUI (Notice|Deprecated)\]/.test(a[0]) || log(lvl, a);
  // An extension awaiting UI that never comes must not stall the rest.
  const within = (p, ms, what) => {
    let t;
    return Promise.race([p, new Promise((r) => (t = setTimeout(() => { console.warn(`timed out: ${typeof what === "function" ? what() : what}`); r(); }, ms)))]).finally(() => clearTimeout(t));
  };
  window.addEventListener("error", (e) => log("error", [e.error || e.message]));
  window.addEventListener("unhandledrejection", (e) => log("error", [e.reason]));

  // Anything we don't implement: a callable, constructible, writable no-op, so an
  // extension touching UI we don't have keeps working instead of throwing at import.
  // Depth-capped: `while (x.parent) x = x.parent` over stubs must end.
  const stub = (path, depth = 0) => {
    if (depth > 3) return undefined;
    const f = function () {};
    return new Proxy(f, {
      get(t, k) {
        if (k === "then") return undefined; // never look like a Promise
        if (k === Symbol.toPrimitive) return () => "";
        if (k === Symbol.iterator) return function* () {};
        if (Reflect.has(t, k)) return t[k];
        return (t[k] = stub(path + "." + String(k), depth + 1));
      },
      apply: () => stub(path + "()", depth + 1),
      construct: () => stub("new " + path, depth + 1) ?? {},
    });
  };
  // A real object whose missing members fall back to stubs.
  const lenient = (obj, path) =>
    new Proxy(obj, {
      get(t, k) {
        if (Reflect.has(t, k) || typeof k === "symbol" || k === "then" || k === "toJSON") {
          const v = Reflect.get(t, k, t);
          // Native methods (EventTarget's) need the real object as `this`, not the proxy.
          return typeof v === "function" && t instanceof EventTarget && v === EventTarget.prototype[k] ? v.bind(t) : v;
        }
        return (t[k] = stub(path + "." + k));
      },
    });

  // ---------------------------------------------------------------- settings
  let values = {};
  const defs = new Map();
  const settingValue = (id) => {
    if (id in values) return values[id];
    const d = defs.get(id)?.defaultValue;
    return typeof d === "function" ? d() : d;
  };
  const runChange = (id, v, old) => {
    try { defs.get(id)?.onChange?.(v, old); } catch (e) { console.error(`onChange ${id}`, e); }
  };
  const setSetting = async (id, v) => {
    const old = settingValue(id);
    values[id] = v;
    runChange(id, v, old);
    post({ t: "setting", id, value: v });
    await fetch(`/api/settings/${encodeURIComponent(id)}`, { method: "POST", body: JSON.stringify(v) });
  };
  const addSetting = (s) => {
    if (!s?.id || defs.has(s.id)) return;
    defs.set(s.id, s);
    runChange(s.id, settingValue(s.id), undefined);
    return { get value() { return settingValue(s.id); }, set value(v) { setSetting(s.id, v); } };
  };
  const serialize = (s) => {
    const val = settingValue(s.id);
    let options = s.options;
    try { if (typeof options === "function") options = options(val); } catch (e) { options = []; }
    const def = typeof s.defaultValue === "function" ? (() => { try { return s.defaultValue(); } catch { return null; } })() : s.defaultValue;
    return {
      id: s.id,
      name: typeof s.name === "string" ? s.name : s.id,
      type: typeof s.type === "string" ? s.type : "custom",
      category: Array.isArray(s.category) ? s.category.map(String) : null,
      tooltip: typeof s.tooltip === "string" ? s.tooltip : "",
      options: Array.isArray(options) ? options.map((o) => (o && typeof o === "object" ? { text: String(o.text ?? o.label ?? o.value), value: o.value } : o)) : null,
      defaultValue: def === undefined ? null : def,
      attrs: s.attrs && typeof s.attrs === "object" ? { min: s.attrs.min, max: s.attrs.max, step: s.attrs.step } : null,
    };
  };
  const settingsUi = lenient({
    addSetting,
    getSettingValue: (id, d) => settingValue(id) ?? d,
    setSettingValue: (id, v) => setSetting(id, v),
    setSettingValueAsync: (id, v) => setSetting(id, v),
    settingsLookup: new Proxy({}, { get: (_, id) => defs.get(id) }),
  }, "app.ui.settings");

  // ---------------------------------------------------------------- api
  class ComfyApi extends EventTarget {
    api_base = "";
    clientId = sessionStorage.getItem("clientId") || (sessionStorage.clientId = crypto.randomUUID());
    initialClientId = this.clientId;
    apiURL(r) { return "/api" + r; }
    fileURL(r) { return r; }
    internalURL(r) { return "/internal" + r; }
    fetchApi(r, o = {}) { return fetch(this.apiURL(r), { cache: "no-cache", ...o }); }
    async getExtensions() { return (await this.fetchApi("/extensions")).json(); }
    async getNodeDefs() { return (await this.fetchApi("/object_info")).json(); }
    async getSettings() { return (await this.fetchApi("/settings")).json(); }
    async getSetting(id) { return (await this.fetchApi(`/settings/${encodeURIComponent(id)}`)).json(); }
    storeSetting(id, v) { return setSetting(id, v); }
    async storeSettings(o) { for (const k in o) await setSetting(k, o[k]); }
    dispatchCustomEvent(type, detail) { return this.dispatchEvent(new CustomEvent(type, { detail })); }
  }
  const api = lenient(new ComfyApi(), "api");

  // ---------------------------------------------------------------- app
  const extensions = [];
  const sidebars = new Map(); // id → tab, shown by `cg.showPanel`
  // `invokeExtensionsAsync`: every extension at once, `app` appended to the arguments.
  // One timer per call (not per extension): this runs for every node type and node.
  const call = (name, ...args) => {
    const pending = new Set();
    const all = Promise.all(extensions.map(async (e) => {
      if (typeof e[name] !== "function") return;
      pending.add(e.name);
      try { await e[name](...args, app); } catch (err) { console.error(`${e.name}.${name}`, err); }
      pending.delete(e.name);
    }));
    return pending.size ? within(all, 5000, () => `${name}: ${[...pending].join(", ")}`) : all;
  };
  const app = lenient({
    api,
    extensions,
    graph: stub("app.graph"),
    canvas: stub("app.canvas"),
    nodeOutputs: {},
    vueAppReady: true,
    registerExtension(ext) {
      if (!ext?.name || extensions.some((e) => e.name === ext.name)) return;
      extensions.push(ext);
      for (const s of ext.settings ?? []) addSetting(s);
    },
    getPreviewFormatParam: () => "",
    getRandParam: () => "&rand=" + Math.random(),
    ui: lenient({ settings: settingsUi, dialog: stub("app.ui.dialog") }, "app.ui"),
    extensionManager: lenient({
      registerSidebarTab: (t) => { if (t?.id && typeof t.render === "function") sidebars.set(t.id, t); },
      setting: { get: settingValue, set: setSetting },
      toast: { add: (t) => log("warn", ["toast", t?.summary, t?.detail]), addAlert: (m) => log("warn", ["alert", m]) },
    }, "app.extensionManager"),
  }, "app");

  // ---------------------------------------------------------------- litegraph mirror
  // A headless stand-in for litegraph: enough node/graph/link behaviour for extension
  // hooks (`onConnectionsChange`, `addInput`, `widgets[0].value` …) to run as they do in
  // the browser. Comfy-GNOME's canvas is the truth; the mirror is rebuilt from it and its
  // structural changes (sockets added/removed/renamed/retyped, widget values) go back.
  window.requestAnimationFrame = (cb) => setTimeout(() => cb(performance.now()), 0); // never drawn: rAF would never fire
  const INPUT = 1, OUTPUT = 2;
  const nid = (id) => (typeof id === "string" && /^-?\d+$/.test(id) ? Number(id) : id);
  // `graph.links` is a Map in current litegraph and a plain object in older code: both work.
  const linkMap = () => {
    const m = new Map();
    return new Proxy(m, {
      get(t, k) {
        if (typeof k === "string" && /^-?\d+$/.test(k)) return t.get(Number(k));
        const v = Reflect.get(t, k, t);
        return typeof v === "function" ? v.bind(t) : v;
      },
      set(t, k, v) { t.set(Number(k), v); return true; },
      deleteProperty(t, k) { t.delete(Number(k)); return true; },
      ownKeys: (t) => [...t.keys()].map(String),
      getOwnPropertyDescriptor: (t, k) => (t.has(Number(k)) ? { value: t.get(Number(k)), enumerable: true, configurable: true, writable: true } : undefined),
    });
  };
  const domHost = () => document.getElementById("cg-dom") ?? document.body.appendChild(Object.assign(document.createElement("div"), { id: "cg-dom", style: "position:fixed;left:-20000px;top:0;width:800px" }));
  class LLink {
    constructor(id, type, origin_id, origin_slot, target_id, target_slot) {
      Object.assign(this, { id, type, origin_id, origin_slot, target_id, target_slot });
    }
    serialize() { return [this.id, this.origin_id, this.origin_slot, this.target_id, this.target_slot, this.type]; }
  }
  class LGraph {
    constructor() {
      this._nodes = [];
      this._nodes_by_id = {};
      this.links = linkMap();
      this.last_link_id = 0;
      this.last_node_id = 0;
      this.extra = {};
      this._groups = [];
      this.subgraphs = new Map();
      this.events = new EventTarget();
    }
    get nodes() { return this._nodes; }
    get groups() { return this._groups; }
    get rootGraph() { return this; }
    getNodeById(id) { return this._nodes_by_id[id] ?? null; }
    getLink(id) { return this.links.get(Number(id)); }
    add(node) {
      if (node.id == null || node.id === -1) node.id = ++this.last_node_id;
      node.graph = this;
      this._nodes.push(node);
      this._nodes_by_id[node.id] = node;
      node.onAdded?.(this);
      return node;
    }
    remove(node) {
      this._nodes.splice(this._nodes.indexOf(node), 1);
      delete this._nodes_by_id[node.id];
      node.onRemoved?.();
    }
    findNodesByType(t) { return this._nodes.filter((n) => n.type === t); }
    findNodeByTitle(t) { return this._nodes.find((n) => n.title === t) ?? null; }
    findNodesByTitle(t) { return this._nodes.filter((n) => n.title === t); }
    computeExecutionOrder() { return this._nodes; }
    serialize() { return { nodes: this._nodes.map((n) => n.serialize()), links: [...this.links.values()].map((l) => l.serialize()) }; }
    setDirtyCanvas() {}
    change() {}
    beforeChange() {}
    afterChange() {}
    updateExecutionOrder() {}
  }
  class LGraphNode {
    constructor(title) {
      this.id = -1;
      this.title = title ?? this.constructor.title ?? "";
      this.inputs = [];
      this.outputs = [];
      this.widgets = [];
      this.properties = {};
      this.flags = {};
      this.pos = [0, 0];
      this.size = [200, 100];
      this.mode = 0;
      this.graph = null;
    }
    addInput(name, type, extra) {
      const s = { name, type, link: null, ...extra };
      this.inputs.push(s);
      this.onInputAdded?.(s);
      return s;
    }
    addOutput(name, type, extra) {
      const s = { name, type, links: null, ...extra };
      this.outputs.push(s);
      this.onOutputAdded?.(s);
      return s;
    }
    addInputs(a) { for (const [n, t, e] of a) this.addInput(n, t, e); }
    addOutputs(a) { for (const [n, t, e] of a) this.addOutput(n, t, e); }
    removeInput(slot) {
      this.disconnectInput(slot, true);
      const [s] = this.inputs.splice(slot, 1);
      for (let i = slot; i < this.inputs.length; i++) {
        const l = this.graph?.getLink(this.inputs[i].link);
        if (l) l.target_slot--;
      }
      this.onInputRemoved?.(slot, s);
    }
    removeOutput(slot) {
      this.disconnectOutput(slot);
      const [s] = this.outputs.splice(slot, 1);
      for (let i = slot; i < this.outputs.length; i++) for (const id of this.outputs[i].links ?? []) { const l = this.graph?.getLink(id); if (l) l.origin_slot--; }
      this.onOutputRemoved?.(slot, s);
    }
    findInputSlot(name, obj) { const i = this.inputs.findIndex((s) => s.name === name); return obj ? this.inputs[i] ?? null : i; }
    findOutputSlot(name, obj) { const i = this.outputs.findIndex((s) => s.name === name); return obj ? this.outputs[i] ?? null : i; }
    findInputSlotByType(t) { return this.inputs.findIndex((s) => s.type === t); }
    findOutputSlotByType(t) { return this.outputs.findIndex((s) => s.type === t); }
    getInputLink(slot) { return this.graph?.getLink(this.inputs[slot]?.link) ?? null; }
    getInputInfo(slot) { return this.inputs[slot] ?? null; }
    getOutputInfo(slot) { return this.outputs[slot] ?? null; }
    getInputNode(slot) { const l = this.getInputLink(slot); return l ? this.graph.getNodeById(l.origin_id) : null; }
    getInputOnSlot(slot) { return this.getInputLink(slot); }
    getOutputNodes(slot) {
      const ids = this.outputs[slot]?.links;
      return ids?.length ? ids.map((id) => this.graph.getNodeById(this.graph.getLink(id)?.target_id)).filter(Boolean) : null;
    }
    getInputData() {}
    getInputDataType(slot) { return this.inputs[slot]?.type; }
    isInputConnected(slot) { return this.inputs[slot]?.link != null; }
    isOutputConnected(slot) { return !!this.outputs[slot]?.links?.length; }
    isAnyOutputConnected() { return this.outputs.some((o) => o.links?.length); }
    getWidgetOnPos() { return null; }
    addWidget(type, name, value, callback, options) {
      if (callback && typeof callback === "object") { options = callback; callback = null; }
      if (typeof options === "string") options = { property: options };
      const w = { type, name, value, callback, options: options ?? {}, y: 0, last_y: 0, computedHeight: 20 };
      this.widgets.push(this.own(w));
      return w;
    }
    // Frontend widgets know their node (`widget.node`, used by Vue widgets); non-enumerable
    // so snapshots/JSON of widgets don't recurse into the node.
    own(w) { if (w && !("node" in w)) Object.defineProperty(w, "node", { value: this, writable: true, configurable: true }); return w; }
    addCustomWidget(w) { this.widgets.push(this.own(w)); return w; }
    addDOMWidget(name, type, element, options = {}) {
      let v;
      // In the document (off-screen), so code checking `document.body.contains(el)` /
      // `isConnected` (autocomplete, availability refresh) treats it as live.
      if (element instanceof Node) domHost().append(element);
      const w = { type, name, element, options, y: 0, computeSize: () => [0, 0], onRemove() { element?.remove?.(); } };
      Object.defineProperty(w, "value", { get: () => (options.getValue ? options.getValue() : v), set: (x) => (options.setValue ? options.setValue(x) : (v = x)), enumerable: true });
      this.widgets.push(this.own(w));
      return w;
    }
    removeWidget(w) { const i = this.widgets.indexOf(w); if (i >= 0) this.widgets.splice(i, 1); w.onRemove?.(); }
    removeWidgetByName(n) { const w = this.widgets.find((w) => w.name === n); if (w) this.removeWidget(w); }
    setProperty(k, v) { const o = this.properties[k]; this.properties[k] = v; this.onPropertyChanged?.(k, v, o); }
    addProperty(k, v) { this.properties[k] = v; }
    setSize(s) { this.size = [s[0], s[1]]; this.onResize?.(this.size); }
    computeSize() { return [this.size[0], this.size[1]]; }
    setDirtyCanvas() {}
    setOutputData() {}
    trigger() {}
    collapse() {}
    getTitle() { return this.title; }
    getBounding() { return [this.pos[0], this.pos[1], this.size[0], this.size[1]]; }
    get boundingRect() { return this.getBounding(); }
    get widgets_values() { return this.widgets.map((w) => w.value); }
    // litegraph `connect`: replaces whatever was on the target input, then notifies both ends.
    ["LGraphNode.prototype.connect"](slot, target, tslot, id) {
      if (typeof slot === "string") slot = this.findOutputSlot(slot);
      if (typeof target === "number" || typeof target === "string") target = this.graph.getNodeById(target);
      if (typeof tslot === "string") tslot = target.findInputSlot(tslot);
      const out = this.outputs[slot], inp = target?.inputs[tslot];
      if (!out || !inp) return null;
      if (inp.link != null) target.disconnectInput(tslot, true);
      const g = this.graph;
      const l = new LLink(id ?? ++g.last_link_id, out.type === "*" ? inp.type : out.type, this.id, slot, target.id, tslot);
      g.last_link_id = Math.max(g.last_link_id, l.id);
      g.links.set(l.id, l);
      (out.links ??= []).push(l.id);
      inp.link = l.id;
      this.onConnectionsChange?.(OUTPUT, slot, true, l, out);
      target.onConnectionsChange?.(INPUT, tslot, true, l, inp);
      return l;
    }
    connect(...a) { return this["LGraphNode.prototype.connect"](...a); }
    connectByType(slot, target, type) { const t = target.findInputSlotByType(type); return t >= 0 ? this.connect(slot, target, t) : null; }
    disconnectInput(slot) {
      if (typeof slot === "string") slot = this.findInputSlot(slot);
      const inp = this.inputs[slot];
      const l = this.graph?.getLink(inp?.link);
      if (!inp || inp.link == null) return false;
      inp.link = null;
      if (!l) return true;
      this.graph.links.delete(l.id);
      const origin = this.graph.getNodeById(l.origin_id);
      const out = origin?.outputs[l.origin_slot];
      if (out?.links) { const i = out.links.indexOf(l.id); if (i >= 0) out.links.splice(i, 1); }
      this.onConnectionsChange?.(INPUT, slot, false, l, inp);
      if (out) origin.onConnectionsChange?.(OUTPUT, l.origin_slot, false, l, out);
      return true;
    }
    disconnectOutput(slot, target) {
      if (typeof slot === "string") slot = this.findOutputSlot(slot);
      for (const id of [...(this.outputs[slot]?.links ?? [])]) {
        const l = this.graph.getLink(id);
        const t = l && this.graph.getNodeById(l.target_id);
        if (t && (!target || t === target)) t.disconnectInput(l.target_slot);
      }
      return true;
    }
    configure(info) {
      for (const k in info) {
        if (k === "properties") { Object.assign(this.properties, info.properties); continue; }
        if (k === "widgets_values" || info[k] == null) continue;
        this[k] = typeof info[k] === "object" ? structuredClone(info[k]) : info[k];
      }
      if (Array.isArray(info.widgets_values)) info.widgets_values.forEach((v, i) => { const w = this.widgets[i]; if (w) w.value = v; });
      this.inputs.forEach((inp, i) => { this.onConnectionsChange?.(INPUT, i, true, this.graph?.getLink(inp.link), inp); this.onInputAdded?.(inp); });
      this.outputs.forEach((out, i) => {
        for (const id of out.links ?? []) this.onConnectionsChange?.(OUTPUT, i, true, this.graph?.getLink(id), out);
        this.onOutputAdded?.(out);
      });
      this.onConfigure?.(info);
    }
    // litegraph's: every widget's value (holes for `serialize: false`), plain JSON.
    serialize() {
      const wv = [];
      this.widgets.forEach((w, i) => { if (w.serialize !== false) wv[i] = w.value ?? null; });
      return JSON.parse(j({
        id: this.id, type: this.type, mode: this.mode, title: this.title, properties: this.properties,
        inputs: this.inputs.map((s) => ({ name: s.name, type: s.type, link: s.link ?? null, ...(s.widget ? { widget: { name: s.widget.name } } : {}), ...(s.label ? { label: s.label } : {}), ...(s.shape != null ? { shape: s.shape } : {}) })),
        outputs: this.outputs.map((s) => ({ name: s.name, type: s.type, links: s.links?.length ? [...s.links] : null })),
        widgets_values: wv,
      }));
    }
  }
  // `scripts/widgets.js` ComfyWidgets: the core widget per input type.
  const customWidgets = {};
  const seedLike = (name, type, o) => o.control_after_generate ?? (type === "INT" && (name === "seed" || name === "noise_seed"));
  const addControl = (node, w) => {
    const c = node.addWidget("combo", "control_after_generate", "randomize", null, { values: ["fixed", "increment", "decrement", "randomize"] });
    w.linkedWidgets = [c];
    return c;
  };
  const ComfyWidgets = {
    INT(node, name, [, o = {}]) { const w = node.addWidget("number", name, o.default ?? 0, null, { ...o, precision: 0 }); if (seedLike(name, "INT", o)) addControl(node, w); return { widget: w }; },
    FLOAT: (node, name, [, o = {}]) => ({ widget: node.addWidget("number", name, o.default ?? 0, null, o) }),
    // Extensions wrap this and hook the element itself (autocompleters read/write
    // `widget.inputEl`, `document.activeElement`), so it is a real, live `<textarea>`.
    STRING(node, name, [, o = {}]) {
      const w = node.addWidget("text", name, o.default ?? "", null, o);
      const el = document.createElement("textarea");
      el.value = w.value;
      el.addEventListener("input", () => { w.value = el.value; });
      w.inputEl = domHost().appendChild(el);
      let raw = w.value;
      Object.defineProperty(w, "value", {
        get: () => raw,
        set: (x) => { raw = x; if (el.value !== String(x ?? "")) el.value = String(x ?? ""); },
        enumerable: true, configurable: true,
      });
      return { widget: w };
    },
    BOOLEAN: (node, name, [, o = {}]) => ({ widget: node.addWidget("toggle", name, o.default ?? false, null, o) }),
    COMBO(node, name, [t, o = {}]) {
      const values = Array.isArray(t) ? t : o.options ?? [];
      const w = node.addWidget("combo", name, o.default ?? values[0], null, { ...o, values });
      if (o.control_after_generate) addControl(node, w);
      return { widget: w };
    },
  };
  // `addInputs` of the web frontend's ComfyNode, one pass at a time: sockets first (pass 0),
  // then a widget per widget input (core or `getCustomWidgets` type) with its socket.
  const addDefWidget = (node, name, spec, required, pass) => {
    const [t, opts = {}] = Array.isArray(spec) ? spec : [spec];
    const type = Array.isArray(t) ? "COMBO" : t;
    const ctor = customWidgets[opts.widgetType ?? type] ?? ComfyWidgets[opts.widgetType ?? type];
    const isWidget = ctor && !opts.forceInput;
    if (pass === 0) { if (!isWidget) node.addInput(name, type, required ? {} : { shape: 7 }); return; }
    if (!isWidget) return;
    const before = new Set(node.widgets);
    let r;
    try { r = ctor(node, name, [t, opts], app); } catch (e) { console.error(`widget ${opts.widgetType ?? type} ${name}`, e); }
    for (const w of node.widgets) if (!before.has(w)) { w.__def = true; node.own(w); }
    const w = r?.widget ?? r;
    if (!opts.socketless) node.addInput(name, type, { widget: { name: w?.name ?? name } });
    for (const k of ["image_upload", "video_upload", "audio_upload"]) if (opts[k]) node.addWidget("button", "upload", "image", null, {}).__def = true;
  };
  const LiteGraph = lenient({
    INPUT, OUTPUT, EVENT: -1, ACTION: -1,
    NODE_TITLE_HEIGHT: 30, NODE_SLOT_HEIGHT: 20, NODE_WIDGET_HEIGHT: 20, NODE_WIDTH: 140, NODE_MIN_WIDTH: 50,
    NODE_TEXT_SIZE: 14, NODE_SUBTEXT_SIZE: 12, NODE_TITLE_COLOR: "#999", NODE_SELECTED_TITLE_COLOR: "#FFF",
    NODE_TEXT_COLOR: "#AAA", NODE_DEFAULT_COLOR: "#333", NODE_DEFAULT_BGCOLOR: "#353535", NODE_DEFAULT_BOXCOLOR: "#666",
    WIDGET_BGCOLOR: "#222", WIDGET_OUTLINE_COLOR: "#666", WIDGET_TEXT_COLOR: "#DDD", WIDGET_SECONDARY_TEXT_COLOR: "#999",
    LINK_COLOR: "#9A9", EVENT_LINK_COLOR: "#A86", CONNECTING_LINK_COLOR: "#AFA",
    BOX_SHAPE: 1, ROUND_SHAPE: 2, CIRCLE_SHAPE: 3, CARD_SHAPE: 4, ARROW_SHAPE: 5, GRID_SHAPE: 6,
    STRAIGHT_LINK: 0, LINEAR_LINK: 1, SPLINE_LINK: 2, ALWAYS: 0, ON_EVENT: 1, NEVER: 2, ON_TRIGGER: 3,
    UP: 1, DOWN: 2, LEFT: 3, RIGHT: 4, CENTER: 5, VERTICAL_LAYOUT: "vertical", HORIZONTAL_LAYOUT: "horizontal",
    registered_node_types: {}, node_types_by_file_extension: {}, Nodes: {}, slot_types_default_in: {}, slot_types_default_out: {},
    LGraph, LGraphNode, LLink,
    registerNodeType(type, cls) {
      cls.type = type;
      if (!Object.hasOwn(cls.prototype, "type")) Object.defineProperty(cls.prototype, "type", { value: type, writable: true, configurable: true });
      this.registered_node_types[type] = cls;
    },
    unregisterNodeType(type) { delete this.registered_node_types[type]; },
    createNode(type, title, opts) {
      const C = this.registered_node_types[type];
      let n;
      try { n = C ? new C(title ?? C.title) : new LGraphNode(title ?? type); } catch (e) { console.error(`create ${type}`, e); n = new LGraphNode(type); }
      n.type = type;
      if (opts) Object.assign(n, opts);
      try { n.onNodeCreated?.(); } catch (e) { console.error(`${type}.onNodeCreated`, e); }
      return n;
    },
    getNodeType(type) { return this.registered_node_types[type]; },
    isValidConnection: (a, b) => !a || !b || a === b || a === "*" || b === "*" || String(a).split(",").some((x) => String(b).split(",").includes(x)),
    closeAllContextMenus() {},
    getTime: () => performance.now(),
  }, "LiteGraph");
  window.LiteGraph = LiteGraph;
  Object.assign(window, { LGraph, LGraphNode, LLink });
  for (const n of ["LGraphCanvas", "LGraphGroup", "ContextMenu", "DragAndScale"]) window[n] = stub(n);

  // `registerNodeDef`: the per-type class every extension's beforeRegisterNodeDef patches.
  app.graph = new LGraph();
  const registerNodeDef = async (name, data) => {
    const ComfyNode = class extends LGraphNode {
      constructor(title) {
        super(title);
        const inp = data.input ?? {};
        const order = data.input_order ?? {};
        for (const pass of [0, 1]) for (const sect of ["required", "optional"]) {
          const specs = inp[sect] ?? {};
          for (const k of order[sect] ?? Object.keys(specs)) if (specs[k]) addDefWidget(this, k, specs[k], sect === "required", pass);
        }
        (data.output ?? []).forEach((t, i) => this.addOutput(data.output_name?.[i] ?? (Array.isArray(t) ? "COMBO" : t), Array.isArray(t) ? "COMBO" : t, data.output_is_list?.[i] ? { shape: 6 } : {}));
        this.serialize_widgets = true;
        call("nodeCreated", this);
      }
    };
    ComfyNode.comfyClass = ComfyNode.prototype.comfyClass = name;
    ComfyNode.nodeData = data;
    ComfyNode.title = data.display_name || name;
    await call("beforeRegisterNodeDef", ComfyNode, data);
    LiteGraph.registerNodeType(name, ComfyNode);
    ComfyNode.category = data.category;
  };

  // ---------------------------------------------------------------- canvas <-> mirror
  // The mirror follows the active tab: Rust sends every real edit (`cg.sync`, before /
  // after graphs in workflow JSON shape) and the mirror replays it the way litegraph would,
  // firing the same hooks (`onConnectionsChange`, widget callbacks, `mode` setters …).
  // Whatever the hooks then change, synchronously or after their own fetches / timers, is
  // posted back as a delta (`{t:"changed"}`) for the canvas to take.
  let cur = null; // key of the mirrored tab
  let queue = Promise.resolve();
  const enqueue = (f) => (queue = queue.then(f).catch((e) => console.error("implant", e)));
  const j = (v) => { try { return JSON.stringify(v ?? null); } catch { return "?"; } };
  const ioJson = (n) => j([
    (n.inputs ?? []).map((i) => [i.name, i.type, i.link ?? null, i.widget?.name ?? null, i.label ?? null]),
    (n.outputs ?? []).map((o) => [o.name, o.type, [...(o.links ?? [])].sort((a, b) => a - b)]),
  ]);
  const linkJson = (l) => ({ id: l.id, origin_id: l.origin_id, origin_slot: l.origin_slot, target_id: l.target_id, target_slot: l.target_slot, type: l.type });
  const lk = (l) => j([String(l.origin_id), l.origin_slot, String(l.target_id), l.target_slot]);
  // What the canvas is known to have, per node: sockets, widget values (only when the
  // mirror's widgets line up 1:1 with the saved values), and the link table.
  const base = new Map();
  let baseLinks = new Map();
  // A combo list the page computes with a getter (`Object.defineProperty(options, "values")`,
  // Impact's wildcard databank) is never visible to the canvas unless we send it: flag it so
  // the first report after the node exists always carries the list.
  const computed = (w) => { try { return !!(w.options && Object.getOwnPropertyDescriptor(w.options, "values")?.get); } catch { return false; } };
  // `sent`: the report just carried the computed lists, so store them as they are — keeping "!"
  // made every later report resend them, re-rendering the canvas forever (drags died mid-way).
  const snap = (n, aligned, sent = false) => ({ io: ioJson(n), w: n.widgets.map((w) => j(w.value)), o: n.widgets.map((w) => j(!sent && computed(w) ? "!" : (w.options?.values ?? null))), aligned });
  const aligned = (n, wv) => Array.isArray(wv) && n.widgets.length === wv.length;
  const mirrorLinks = () => new Map([...app.graph.links.values()].map((l) => [l.id, lk(l)]));
  let lastHits = "";
  const hits = () => {
    const els = [...(document.getElementById("cg-layer")?.children ?? []), ...document.querySelectorAll("body>[data-cg-ok]")];
    const r = els.flatMap((e) => {
      const b = e.getBoundingClientRect();
      return b.width > 0 && b.height > 0 && getComputedStyle(e).visibility !== "hidden" ? [[b.left, b.top, b.width, b.height].map(Math.round)] : [];
    });
    const k = JSON.stringify(r);
    if (k !== lastHits) { lastHits = k; post({ t: "hits", rects: r }); }
  };
  // The page is drawn over the canvas now: stray elements other extensions put in <body>
  // stay hidden, except popups a DOM widget opens (menus, tooltips) right after it was used.
  let layerUsed = -1e9;
  // Text boxes that come from a node's DOM widget are drawn by the page, but must look like the
  // canvas' own multiline rows (widgets.rs: Sans 12px, 14px pitch, 8px inset, #222 on #ddd).
  document.head.appendChild(Object.assign(document.createElement("style"), { textContent: ":root{--comfy-input-bg:#222;--input-text:#ddd;--comfy-menu-bg:#353535;--fg-color:#fff;--bg-color:#202020;--border-color:#4e4e4e;--descrip-text:#999;--font-mono:ui-monospace,monospace;--comfy-textarea-font-size:10px;color-scheme:dark} html,body{background:transparent!important;margin:0;overflow:hidden!important} body>*:not(#cg-layer):not(#cg-panel):not(#cg-seq):not([data-cg-ok]){visibility:hidden!important} #cg-layer textarea,#cg-layer .comfy-multiline-input{background-color:#222;color:#ddd;border:none;border-radius:0;box-sizing:border-box;font-family:sans-serif;font-size:12px;line-height:14px;padding:5px 8px 24px 8px;margin:0;resize:none;outline:none;caret-color:#ddd} #cg-layer textarea:focus{outline:none}" }));
  // LoRA Manager's hover preview is a <video> for .mp4 previews. WebKit here often cannot decode
  // it (no H.264 in the sandbox) and draws its "?" placeholder in a bordered box that then
  // floats at the last pointer spot, because the page never sees the pointer leave. Media
  // errors don't bubble, so listen in the capture phase and drop the tooltip.
  document.addEventListener("error", (e) => { const t = e.target?.closest?.(".lm-tooltip"); if (t) { t.style.display = "none"; delete t.dataset.cgOk; } }, true);
  for (const t of ["pointerdown", "pointerover", "focusin", "keydown"]) document.addEventListener(t, (e) => { if (e.target?.closest?.("#cg-layer,[data-cg-ok]")) layerUsed = performance.now(); }, true);
  const watchBody = () => new MutationObserver((ms) => {
    for (const m of ms) for (const n of m.addedNodes) if (n.parentNode === document.body && n instanceof HTMLElement && performance.now() - layerUsed < 3000) n.dataset.cgOk = "";
  }).observe(document.body, { childList: true });
  // Popups that already live in <body> and are only shown on demand (LoRA Manager's autocomplete
  // list is created once, then toggled): a style/class flip right after the user used a widget.
  const watchShow = () => new MutationObserver((ms) => {
    if (performance.now() - layerUsed > 3000) return;
    for (const m of ms) if (m.target.parentNode === document.body && !m.target.id.startsWith("cg-") && (!m.oldValue || /display:\s*none|visibility:\s*hidden|hidden/.test(m.oldValue)) && getComputedStyle(m.target).display !== "none") m.target.dataset.cgOk = "";
  }).observe(document.body, { attributes: true, attributeOldValue: true, attributeFilter: ["style", "class"], subtree: true });
  const watchAll = () => { watchBody(); watchShow(); };
  if (document.body) watchAll(); else document.addEventListener("DOMContentLoaded", watchAll);
  new MutationObserver(() => origTimeout(hits, 0)).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["style", "class"] });
  let lastDom = "";
  // DOM widgets (`addDOMWidget` with an element that isn't a plain text box's `inputEl`):
  // the canvas leaves a row for them and the page draws them there (see `cg.layout`).
  const isDom = (w) => w.element instanceof Element && w.element !== w.inputEl;
  const domMinHeight = (w) => {
    let h;
    try { h = w.options?.getMinHeight?.() ?? parseFloat(w.element.style.getPropertyValue("--comfy-widget-min-height")); } catch {}
    return Number.isFinite(h) && h > 0 ? h : 120;
  };
  const report = () => {
    if (cur == null) return;
    const t0 = performance.now();
    try { return report1(); } finally { const dt = performance.now() - t0; if (dt > 3) console.info(`report ${dt.toFixed(1)} ms`); }
  };
  const report1 = () => {
    const nodes = [];
    for (const n of app.graph._nodes) {
      const b = base.get(String(n.id));
      if (!b) continue;
      const d = { id: n.id };
      // Choices the page computed at runtime (Impact's wildcard databank, dynamic combos):
      // the canvas has no other way to learn them.
      const o = n.widgets.flatMap((w, i) => {
        const v = j(w.options?.values ?? null);
        return v !== b.o[i] && Array.isArray(w.options?.values) ? [[i, w.name ?? String(i), JSON.parse(v)]] : [];
      });
      if (o.length) d.options = o;
      const s = n.serialize();
      if (ioJson(s) !== b.io) Object.assign(d, { inputs: s.inputs, outputs: s.outputs });
      if (b.aligned) {
        const w = n.widgets.flatMap((w, i) => (!w.__opaque && w.serialize !== false && j(w.value) !== b.w[i] ? [[i, JSON.parse(j(w.value))]] : []));
        if (w.length) d.widgets = w;
      }
      base.set(String(n.id), { ...snap(n, b.aligned, true) });
      if (Object.keys(d).length > 1) nodes.push(d);
    }
    const now = mirrorLinks();
    const add = [...app.graph.links.values()].filter((l) => baseLinks.get(l.id) !== lk(l)).map(linkJson);
    const remove = [...baseLinks.keys()].filter((id) => now.get(id) !== baseLinks.get(id));
    baseLinks = now;
    // Nodes whose UI is DOM the canvas cannot draw: the shell offers "Open node UI" for them.
    const dom = JSON.stringify(Object.fromEntries(app.graph._nodes.flatMap((n) => {
      const ws = n.widgets.flatMap((w, i) => isDom(w) ? [[w.name, domMinHeight(w), i]] : []);
      return ws.length ? [[`${n.id}|${n.type}`, ws]] : [];
    })));
    if (dom !== lastDom) { lastDom = dom; post({ t: "dom", nodes: JSON.parse(dom) }); }
    if (nodes.length || add.length || remove.length) {
      const m = { t: "changed", key: cur, nodes, links: { add, remove } };
      post(m);
    }
  };
  // Hooks finishing later (fetch → widget update, debounced syncs): report once things settle.
  const origTimeout = window.setTimeout.bind(window), origFetch = window.fetch.bind(window);
  let pending = 0;
  // While a pointer is held down inside the page (scrubbing a LoRA strength, dragging a row) the
  // mirror isn't synced: every sync re-renders the canvas (~35 ms), which made drags stutter.
  // One sync follows the release.
  let held = 0;
  for (const t of ["pointerdown"]) window.addEventListener(t, () => { held++; }, true);
  for (const t of ["pointerup", "pointercancel"]) window.addEventListener(t, () => { held = Math.max(0, held - 1); if (!held) origTimeout(poke, 0); }, true);
  const poke = () => { if (cur != null && !pending && !held) pending = origTimeout(() => { pending = 0; enqueue(report); }, 50); };
  window.setTimeout = (f, ms, ...a) => origTimeout(typeof f === "function" ? () => { try { f(...a); } finally { poke(); } } : f, ms);
  window.fetch = (...a) => {
    const p = origFetch(...a);
    p.finally(() => origTimeout(poke, 0)).catch(() => {});
    return p;
  };
  const dropGraph = () => {
    for (const n of [...app.graph._nodes]) {
      for (const w of n.widgets) { try { w.onRemove?.(); } catch {} }
      try { app.graph.remove(n); } catch (e) { console.error(`${n.type}.onRemoved`, e); }
    }
  };
  const stripLinks = (info) => ({ ...info, id: nid(info.id), inputs: info.inputs?.map((i) => ({ ...i, link: null })), outputs: info.outputs?.map((o) => ({ ...o, links: null })) });
  // Named for the extensions that look for it on the stack (`stack.includes('loadGraphData')`).
  const loaders = {
    async loadGraphData(g) {
      dropGraph();
      const graph = new LGraph();
      app.graph = graph;
      app.configuringGraph = true;
      try {
        for (const l of g.links) {
          graph.links.set(l.id, new LLink(l.id, l.type, nid(l.origin_id), l.origin_slot, nid(l.target_id), l.target_slot));
          graph.last_link_id = Math.max(graph.last_link_id, l.id);
        }
        const made = [];
        for (const info of g.nodes) {
          const n = LiteGraph.createNode(info.type);
          n.id = nid(info.id);
          graph.last_node_id = Math.max(graph.last_node_id, Number(n.id) || 0);
          graph.add(n);
          made.push([n, info]);
        }
        for (const [n, info] of made) {
          try { n.configure({ ...info, id: nid(info.id) }); } catch (e) { console.error(`${info.type}.configure`, e); }
        }
        for (const [n] of made) await call("loadedGraphNode", n);
      } finally {
        app.configuringGraph = false;
      }
      for (const n of graph._nodes) { try { n.onAfterGraphConfigured?.(); } catch (e) { console.error(`${n.type}.onAfterGraphConfigured`, e); } }
      await call("afterConfigureGraph", []);
      await new Promise((r) => origTimeout(r, 0)); // the rAF work queued above
      // What the load hooks did is the baseline, not a change (the canvas has the file).
      base.clear();
      const by = new Map(g.nodes.map((n) => [String(n.id), n]));
      for (const n of graph._nodes) base.set(String(n.id), snap(n, aligned(n, by.get(String(n.id))?.widgets_values)));
      baseLinks = mirrorLinks();
    },
  };
  // Replay one canvas edit (`pre` → `post`) on the mirror, in litegraph's order.
  const replay = async (pre, post) => {
    const g = app.graph;
    const P = new Map(pre.nodes.map((n) => [String(n.id), n])), Q = new Map(post.nodes.map((n) => [String(n.id), n]));
    const PL = new Map(pre.links.map((l) => [l.id, l])), QL = new Map(post.links.map((l) => [l.id, l]));
    const removed = pre.links.filter((l) => !QL.has(l.id) || lk(QL.get(l.id)) !== lk(l));
    const added = post.links.filter((l) => !PL.has(l.id) || lk(PL.get(l.id)) !== lk(l));
    const tk = (l) => `${l.target_id}:${l.target_slot}`;
    const replaced = new Set(added.map(tk)); // `connect` onto a used input drops the old wire itself
    const guard = (what, f) => { try { f(); } catch (e) { console.error(what, e); } };
    for (const r of removed) {
      const l = g.getLink(r.id);
      if (!l || (replaced.has(tk(r)) && QL.get(r.id) == null && P.has(String(r.target_id)) && Q.has(String(r.target_id)))) continue;
      guard("disconnect", () => g.getNodeById(l.target_id)?.disconnectInput(l.target_slot));
    }
    for (const [id] of P) if (!Q.has(id)) { const n = g.getNodeById(nid(id)); if (n) guard("remove", () => { for (const w of n.widgets) w.onRemove?.(); g.remove(n); base.delete(id); }); }
    for (const [id, info] of Q) {
      if (P.has(id)) continue;
      guard(`add ${info.type}`, () => {
        const n = LiteGraph.createNode(info.type);
        n.id = nid(id);
        g.last_node_id = Math.max(g.last_node_id, Number(n.id) || 0);
        g.add(n);
        n.configure(stripLinks(info));
        base.set(id, { ...snap(n, aligned(n, info.widgets_values)), io: ioJson(info) });
      });
      await call("nodeCreated", g.getNodeById(nid(id))); // a pasted / added node, as the web UI does on add
    }
    for (const [id, q] of Q) {
      const p = P.get(id), n = g.getNodeById(nid(id)), b = base.get(id);
      if (!p || !n || !b) continue;
      const strip = (x) => j([(x.inputs ?? []).map((i) => [i.name, i.type, i.widget?.name, i.label]), (x.outputs ?? []).map((o) => [o.name, o.type])]);
      // Sockets the canvas itself reshaped (convert to input, subgraph IO …).
      if (strip(p) !== strip(q)) {
        n.inputs = structuredClone(q.inputs ?? []).map((i) => ({ ...i, link: g.getLink(i.link) ? i.link : null }));
        n.outputs = structuredClone(q.outputs ?? []).map((o) => ({ ...o, links: (o.links ?? []).filter((x) => g.getLink(x)) }));
      }
      if (b.aligned && Array.isArray(q.widgets_values)) {
        q.widgets_values.forEach((v, i) => {
          const w = n.widgets[i];
          if (!w || j(v) === j(p.widgets_values?.[i])) return;
          const old = w.value;
          b.w[i] = j(v);
          guard(`${n.type}.${w.name}`, () => {
            w.value = v;
            if (j(w.value) !== j(v)) w.__opaque = true; // a widget whose value we can't round-trip
            w.callback?.(w.value, app.canvas, n, [0, 0], {});
            n.onWidgetChanged?.(w.name, w.value, old, w);
          });
        });
      }
      if (p.mode !== q.mode) guard(`${n.type}.mode`, () => { n.mode = q.mode; });
      if (j(p.properties) !== j(q.properties)) Object.assign(n.properties, q.properties ?? {});
      if (j(p.title) !== j(q.title) && q.title != null) n.title = q.title;
      b.io = ioJson(q);
    }
    for (const a of added) {
      const o = g.getNodeById(nid(a.origin_id)), t = g.getNodeById(nid(a.target_id));
      guard("connect", () => o?.connect(a.origin_slot, t, a.target_slot, a.id));
    }
    baseLinks = new Map(post.links.map((l) => [l.id, lk(l)]));
    await new Promise((r) => origTimeout(r, 0));
  };

  const modules = {
    app: { app, ComfyApp: stub("ComfyApp"), sanitizeNodeName: (s) => String(s).replace(/[&<>"'`=]/g, "") },
    api: { api, ComfyApi },
    ui: { ComfyDialog: stub("ComfyDialog"), $el: stub("$el") },
    widgets: { ComfyWidgets, addValueControlWidget: (node, w) => addControl(node, w), addValueControlWidgets: (node, w) => [addControl(node, w)] },
    utils: { clone: (o) => structuredClone(o), addStylesheet: () => {}, applyTextReplacements: (_, v) => v, getStorageValue: (k) => localStorage.getItem(k), setStorageValue: (k, v) => localStorage.setItem(k, v) },
  };
  window.comfyAPI = new Proxy(modules, {
    get: (t, k) => (typeof k === "symbol" ? undefined : lenient((t[k] ??= {}), String(k))),
  });
  // `scripts/ui.js` exports the real `$el`: a tiny element builder, used at import time.
  modules.ui.$el = (tag, props, children) => {
    if (!Array.isArray(props) && typeof props !== "object") { children = props; props = {}; }
    if (Array.isArray(props)) { children = props; props = {}; }
    const [name, ...cls] = String(tag).split(".");
    const el = document.createElement(name || "div");
    if (cls.length) el.className = cls.join(" ");
    const { parent, $, style, dataset, ...rest } = props || {};
    Object.assign(el, rest);
    if (style) Object.assign(el.style, style);
    if (children) for (const c of [].concat(children)) el.append(c instanceof Node ? c : String(c));
    parent?.append?.(el);
    $?.(el);
    return el;
  };

  // ---------------------------------------------------------------- lifecycle
  let startedOk;
  const started = new Promise((r) => (startedOk = r));
  window.cg = {
    // `core`: [{id, url, export}] options the static scan found as imports it can't read.
    async start(core) {
      values = await api.getSettings().catch(() => ({}));
      const list = await api.getExtensions().catch(() => []);
      log("info", [`loading ${list.length} extension files`]);
      await Promise.all(list.map((p) => within(import(p), 10000, p).catch((e) => console.error(`load ${p}`, e))));
      log("info", ["imports done"]);
      await call("init");
      // `getCustomWidgets`: widget types extensions add (LoRA Manager's autocomplete text …),
      // registered before any node definition like the web UI does.
      for (const e of extensions) {
        if (typeof e.getCustomWidgets !== "function") continue;
        try { Object.assign(customWidgets, await within(e.getCustomWidgets(app), 5000, `${e.name}.getCustomWidgets`)); } catch (err) { console.error(`${e.name}.getCustomWidgets`, err); }
      }
      log("info", ["init done"]);
      const nodeDefs = await api.getNodeDefs().catch(() => ({}));
      await call("addCustomNodeDefs", nodeDefs);
      for (const [name, data] of Object.entries(nodeDefs)) {
        try { await registerNodeDef(name, data); } catch (e) { console.error(`register ${name}`, e); }
      }
      log("info", [`${Object.keys(nodeDefs).length} node types registered`]);
      await call("registerCustomNodes");
      await call("setup");
      const coreOptions = {};
      for (const c of core || []) {
        try {
          const v = (await import(c.url))[c.export];
          coreOptions[c.id] = (Array.isArray(v) ? v : Object.values(v)).map((o) => (o && typeof o === "object" ? { text: String(o.text ?? o.value), value: o.value } : o));
        } catch (e) { console.error(`core options ${c.id}`, e); }
      }
      startedOk();
      // Non-node surface: commands (+ their keybindings) the GTK shell lists and binds.
      const commands = extensions.flatMap((e) => (e.commands ?? []).filter((c) => c?.id).map((c) => ({ id: c.id, label: String(c.label ?? c.menubarLabel ?? c.id), ext: e.name })));
      // Action-bar buttons become commands too (`btn:<ext>:<i>`); custom bottom-panel tabs
      // render like sidebar tabs.
      extensions.forEach((e) => (e.actionBarButtons ?? []).forEach((b, i) => typeof b?.onClick === "function" && commands.push({ id: `btn:${e.name}:${i}`, label: String(b.label ?? b.tooltip ?? `${e.name} button`), ext: e.name })));
      for (const e of extensions) for (const t of e.bottomPanelTabs ?? []) if (t?.id && typeof t.render === "function") sidebars.set(t.id, t);
      const keybindings = extensions.flatMap((e) => (e.keybindings ?? []).filter((k) => k?.commandId && k.combo?.key).map((k) => ({ id: k.commandId, ...k.combo })));
      post({ t: "ready", settings: [...defs.values()].map(serialize), coreOptions, extensions: extensions.map((e) => e.name), commands, keybindings, sidebars: [...sidebars.values()].map((t) => ({ id: t.id, title: String(t.title ?? t.tooltip ?? t.id) })) });
    },
    // An extension sidebar tab, drawn by its own `render(el)` into the page the GTK shell shows.
    showPanel(id) {
      const t = sidebars.get(id);
      let p = document.getElementById("cg-panel");
      if (!p) {
        p = Object.assign(document.createElement("div"), { id: "cg-panel" });
        p.style.cssText = "position:fixed;inset:0;overflow:auto;background:#202020;color:#ddd;font:14px sans-serif;display:none";
        document.body.appendChild(p);
      }
      if (!t) return;
      for (const c of p.children) c.style.display = c.dataset.id === id ? "" : "none";
      if (![...p.children].some((c) => c.dataset.id === id)) {
        const el = Object.assign(document.createElement("div"), {});
        el.dataset.id = id;
        p.appendChild(el);
        try { t.render(el); } catch (err) { console.error(`sidebar ${id}`, err); }
      }
      p.style.display = "block";
    },
    // The canvas drew a frame: put every visible DOM widget over its row, in screen pixels.
    // items: [[nodeId, widgetName, x, y, w, h, zoom]] (x/y screen, w/h canvas units).
    layout(items, seq) {
      let layer = document.getElementById("cg-layer");
      if (!layer) {
        layer = Object.assign(document.createElement("div"), { id: "cg-layer" });
        layer.style.cssText = "position:fixed;inset:0;pointer-events:none;overflow:hidden";
        document.body.appendChild(layer);
        // Edits inside a DOM widget: report once they settle, like any other hook change.
        for (const t of ["pointerup", "change", "input", "keyup"]) layer.addEventListener(t, poke, true);
        // Focusing a box inside makes the browser scroll the (overflow:hidden) layer to reveal it,
        // which slides every widget sideways: pin it.
        layer.addEventListener("scroll", () => { layer.scrollLeft = 0; layer.scrollTop = 0; });
      }
      const shown = new Set();
      for (const [id, name, x, y, w, h, z] of items) {
        const wd = app.graph.getNodeById(nid(id))?.widgets.find((x) => x.name === name && isDom(x));
        if (!wd) continue;
        const el = wd.element;
        shown.add(el);
        if (el.parentNode !== layer) layer.appendChild(el);
        Object.assign(el.style, { position: "absolute", left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px`, transform: `scale(${z})`, transformOrigin: "0 0", pointerEvents: "auto", display: "", boxSizing: "border-box" });
        // Content taller than the row must not spill over the nodes below it: scroll inside instead.
        if (getComputedStyle(el).overflowY === "visible") el.style.overflow = "auto";
      }
      for (const el of [...layer.children]) if (!shown.has(el)) domHost().append(el);
      hits();
      // Stamp the frame: pixel (0,0) carries `seq`, the shell reads it back to know which
      // layout it is drawing (implant/mod.rs `Shift`) and clips that pixel away.
      let m = document.getElementById("cg-seq");
      if (!m) {
        m = Object.assign(document.createElement("div"), { id: "cg-seq" });
        m.style.cssText = "position:fixed;left:0;top:0;width:1px;height:1px;z-index:2147483647;pointer-events:none";
        document.body.appendChild(m);
      }
      m.style.background = `rgb(${seq & 255},${(seq >> 8) & 255},90)`;
    },
    // The user pressed on the canvas (outside every page element): in a browser the page would
    // see that click, and its open menus / pickers close on it. Replay it on <html>.
    outside() {
      document.activeElement?.blur?.();
      const o = { bubbles: true, cancelable: true, view: window, clientX: -1, clientY: -1 };
      for (const t of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) document.documentElement.dispatchEvent(new (t.startsWith("pointer") ? PointerEvent : MouseEvent)(t, o));
    },
    hidePanel() {
      const p = document.getElementById("cg-panel");
      if (p) p.style.display = "none";
    },
    // A command picked in the GTK shell (menu or shortcut).
    async runCommand(id) {
      const b = /^btn:(.*):(\d+)$/.exec(id);
      if (b) { try { await extensions.find((e) => e.name === b[1])?.actionBarButtons?.[+b[2]]?.onClick?.(); } catch (err) { console.error(`button ${id}`, err); } return; }
      for (const e of extensions) {
        const c = (e.commands ?? []).find((x) => x?.id === id);
        if (c) { try { await c.function?.(); } catch (err) { console.error(`command ${id}`, err); } return; }
      }
    },
    // The GTK settings page changed (and already stored) a value.
    applySetting(id, v) {
      const old = settingValue(id);
      values[id] = v;
      runChange(id, v, old);
    },
    // Fresh options for one setting (they may depend on the current value).
    options(id) {
      const s = defs.get(id);
      return s ? serialize(s).options : null;
    },
    // The active tab (`key`) is now `post`; `pre` is what was sent last for it (null:
    // a different tab, or the canvas replaced its graph wholesale — undo, reload).
    sync(key, pre, post) {
      enqueue(async () => {
        await started;
        if (cur !== key || !pre) {
          cur = null; // loading isn't an edit: nothing to report until it's done
          await loaders.loadGraphData(post);
          cur = key;
          report(); // what the load hooks reshaped
          return;
        }
        await replay(pre, post);
        report();
      });
    },
    // Backend websocket events, so `api.addEventListener("x", …)` hooks fire.
    event(type, detail) { api.dispatchEvent(new CustomEvent(type, { detail })); },
    // One backend websocket text frame, as `api` would have received it.
    ws(raw) {
      let m;
      try { m = JSON.parse(raw); } catch { return; }
      if (!m?.type) return;
      enqueue(async () => {
        await started;
        if (m.type === "executed" && cur != null) {
          const id = m.data?.display_node ?? m.data?.node;
          const n = app.graph.getNodeById(nid(String(id)));
          try { n?.onExecuted?.(m.data.output); } catch (e) { console.error(`${n.type}.onExecuted`, e); }
        }
        api.dispatchEvent(new CustomEvent(m.type, { detail: m.data }));
        poke();
      });
    },
  };
})();
