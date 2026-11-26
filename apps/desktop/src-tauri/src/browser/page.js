// Ghost's page agent (docs/superpowers/plans/2026-11-09-browser.md), injected into every top
// document of the browser beside the chat before the page's own scripts run. It reads the page
// for agents (an outline with refs), acts on it where the engine has no trusted input of its own,
// keeps the console, forwards the browser's own shortcuts to Ghost, and runs Annotate's picker.
// It talks to Ghost only through `window.ipc` (wry's message handler), which accepts a few typed
// messages and trusts none of their contents.
(() => {
  if (window.top !== window || Object.hasOwn(window, "__auraBrowser")) return;

  // The page may replace these later; keep the originals.
  const post = (() => {
    const ipc = window.ipc;
    return (message) => {
      try {
        if (ipc && typeof ipc.postMessage === "function") ipc.postMessage(JSON.stringify(message));
      } catch {
        // The host is gone (the tab is closing).
      }
    };
  })();
  const getRect = Element.prototype.getBoundingClientRect;
  const getStyle = window.getComputedStyle.bind(window);
  const fromPoint = document.elementsFromPoint.bind(document);
  const stringify = JSON.stringify;
  const now = () => Date.now();

  // ---------------------------------------------------------------- console

  const logs = [];
  const LOG_LIMIT = 300;
  const describe = (value, depth = 0) => {
    if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
    if (typeof value === "string") return value;
    if (value === undefined) return "undefined";
    if (typeof value === "function") return `[function ${value.name || "anonymous"}]`;
    if (typeof value === "symbol" || typeof value === "bigint") return String(value);
    if (value instanceof Node) return `[${value.nodeName.toLowerCase()}]`;
    if (depth > 0) return String(value);
    try {
      const seen = new WeakSet();
      return stringify(value, (_key, item) => {
        if (typeof item === "object" && item !== null) {
          if (seen.has(item)) return "[circular]";
          seen.add(item);
        }
        if (typeof item === "bigint") return String(item);
        return item;
      });
    } catch {
      return String(value);
    }
  };
  const record = (level, args) => {
    let text = "";
    try {
      text = Array.from(args, (arg) => describe(arg)).join(" ");
    } catch {
      text = "[unreadable]";
    }
    if (text.length > 2000) text = `${text.slice(0, 2000)}…`;
    logs.push({ level, text, at: now() });
    if (logs.length > LOG_LIMIT) logs.splice(0, logs.length - LOG_LIMIT);
  };
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level];
    if (typeof original !== "function") continue;
    const wrapped = function (...args) {
      record(level === "log" ? "info" : level, args);
      return original.apply(this, args);
    };
    try {
      Object.defineProperty(wrapped, "name", { value: original.name });
      wrapped.toString = () => original.toString();
      console[level] = wrapped;
    } catch {
      // A console that cannot be wrapped keeps its own.
    }
  }
  window.addEventListener(
    "error",
    (event) => {
      if (event.target && event.target !== window && event.target instanceof Element) {
        const source = event.target.getAttribute("src") || event.target.getAttribute("href") || "";
        record("error", [`Failed to load ${event.target.nodeName.toLowerCase()} ${source}`]);
        return;
      }
      const where = event.filename ? ` (${event.filename}:${event.lineno}:${event.colno})` : "";
      record("error", [`${event.message}${where}`]);
    },
    true,
  );
  window.addEventListener("unhandledrejection", (event) => {
    record("error", ["Unhandled promise rejection:", event.reason]);
  });

  // ---------------------------------------------------------------- network (no wrapping)

  const network = () => {
    const entries = [];
    try {
      const navigation = performance.getEntriesByType("navigation")[0];
      if (navigation)
        entries.push({
          url: navigation.name,
          type: "document",
          status: navigation.responseStatus ?? null,
          ms: Math.round(navigation.duration),
          bytes: navigation.transferSize ?? null,
        });
      for (const entry of performance.getEntriesByType("resource").slice(-300))
        entries.push({
          url: entry.name,
          type: entry.initiatorType,
          status: entry.responseStatus ?? null,
          ms: Math.round(entry.duration),
          bytes: entry.transferSize ?? null,
        });
    } catch {
      // No timing API.
    }
    return entries;
  };

  // ---------------------------------------------------------------- the outline

  const INTERACTIVE = new Set([
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "switch",
    "slider",
    "spinbutton",
    "treeitem",
  ]);
  const SKIP = new Set([
    "SCRIPT",
    "STYLE",
    "NOSCRIPT",
    "TEMPLATE",
    "META",
    "LINK",
    "HEAD",
    "BASE",
    "TITLE",
  ]);
  const INPUT_ROLES = {
    button: "button",
    submit: "button",
    reset: "button",
    image: "button",
    checkbox: "checkbox",
    radio: "radio",
    range: "slider",
    number: "spinbutton",
    search: "searchbox",
    email: "textbox",
    tel: "textbox",
    url: "textbox",
    text: "textbox",
    password: "textbox",
    date: "textbox",
    "datetime-local": "textbox",
    month: "textbox",
    week: "textbox",
    time: "textbox",
    color: "button",
    file: "button",
  };

  const roleOf = (element) => {
    const explicit = element.getAttribute("role");
    if (explicit) return explicit.split(/\s+/)[0];
    const tag = element.localName;
    switch (tag) {
      case "a":
      case "area":
        return element.hasAttribute("href") ? "link" : null;
      case "button":
      case "summary":
        return "button";
      case "input": {
        const type = (element.getAttribute("type") || "text").toLowerCase();
        if (type === "hidden") return null;
        if ((type === "text" || type === "search") && element.hasAttribute("list"))
          return "combobox";
        return INPUT_ROLES[type] || "textbox";
      }
      case "textarea":
        return "textbox";
      case "select":
        return element.multiple || element.size > 1 ? "listbox" : "combobox";
      case "option":
        return "option";
      case "img":
        return element.getAttribute("alt") === "" ? null : "img";
      case "h1":
      case "h2":
      case "h3":
      case "h4":
      case "h5":
      case "h6":
        return "heading";
      case "nav":
        return "navigation";
      case "main":
        return "main";
      case "header":
        return element.closest("article, aside, main, nav, section") ? null : "banner";
      case "footer":
        return element.closest("article, aside, main, nav, section") ? null : "contentinfo";
      case "aside":
        return "complementary";
      case "form":
        return "form";
      case "dialog":
        return "dialog";
      case "ul":
      case "ol":
        return "list";
      case "li":
        return "listitem";
      case "table":
        return "table";
      case "tr":
        return "row";
      case "th":
        return "columnheader";
      case "td":
        return "cell";
      case "fieldset":
        return "group";
      case "section":
        return element.hasAttribute("aria-label") || element.hasAttribute("aria-labelledby")
          ? "region"
          : null;
      case "article":
        return "article";
      case "progress":
        return "progressbar";
      case "iframe":
        return "iframe";
      default:
        if (element.isContentEditable && !element.parentElement?.isContentEditable)
          return "textbox";
        return null;
    }
  };

  const clean = (text, limit = 120) => {
    const value = (text || "").replace(/\s+/g, " ").trim();
    return value.length > limit ? `${value.slice(0, limit)}…` : value;
  };
  // The text as a person reads it (innerText), not the source's (textContent).
  // biome-ignore lint/performance/useDomNodeTextContent: the rendered text is the point
  const textOf = (element) => clean(element.innerText ?? element.textContent ?? "");
  const labelled = (element) => {
    const ids = element.getAttribute("aria-labelledby");
    if (ids) {
      const text = ids
        .split(/\s+/)
        .map((id) => element.ownerDocument.getElementById(id))
        .filter(Boolean)
        .map((node) => textOf(node))
        .join(" ");
      if (text) return text;
    }
    const aria = element.getAttribute("aria-label");
    if (aria) return clean(aria);
    return "";
  };
  const nameOf = (element, role) => {
    const named = labelled(element);
    if (named) return named;
    const tag = element.localName;
    if (tag === "input" || tag === "textarea" || tag === "select") {
      if (element.id) {
        const label = element.ownerDocument.querySelector(`label[for="${CSS.escape(element.id)}"]`);
        if (label) return textOf(label);
      }
      const wrapping = element.closest("label");
      if (wrapping) return textOf(wrapping);
      const type = (element.getAttribute("type") || "").toLowerCase();
      if (type === "submit" || type === "button" || type === "reset")
        return clean(element.value || type);
      return clean(
        element.getAttribute("placeholder") ||
          element.getAttribute("title") ||
          element.getAttribute("name") ||
          "",
      );
    }
    if (tag === "img" || tag === "area") return clean(element.getAttribute("alt") || "");
    if (tag === "iframe") return clean(element.getAttribute("title") || "");
    if (
      role &&
      (INTERACTIVE.has(role) ||
        role === "heading" ||
        role === "listitem" ||
        role === "cell" ||
        role === "columnheader" ||
        role === "option")
    ) {
      const text = textOf(element);
      if (text) return text;
    }
    return clean(element.getAttribute("title") || "");
  };

  // Stable refs for as long as an element lives.
  let refCount = 0;
  const refs = new Map();
  const refOf = new WeakMap();
  const refFor = (element) => {
    let ref = refOf.get(element);
    if (!ref) {
      refCount += 1;
      ref = `e${refCount}`;
      refOf.set(element, ref);
    }
    refs.set(ref, new WeakRef(element));
    return ref;
  };
  const byRef = (ref) => {
    const element = refs.get(ref)?.deref();
    return element?.isConnected ? element : null;
  };

  const hidden = (element, style) =>
    element.hidden ||
    element.getAttribute("aria-hidden") === "true" ||
    style.display === "none" ||
    style.visibility === "hidden" ||
    style.visibility === "collapse" ||
    (style.opacity === "0" && style.pointerEvents === "none");
  const clickable = (element, style) =>
    style.cursor === "pointer" &&
    element.localName !== "html" &&
    element.localName !== "body" &&
    getStyle(element.parentElement || element).cursor !== "pointer";

  const snapshot = (scope) => {
    const lines = [];
    let chars = 0;
    let truncated = false;
    const LINE_LIMIT = 1500;
    const CHAR_LIMIT = 48000;
    const push = (depth, text) => {
      if (truncated) return;
      const line = `${"  ".repeat(depth)}- ${text}`;
      if (lines.length >= LINE_LIMIT || chars + line.length > CHAR_LIMIT) {
        truncated = true;
        return;
      }
      lines.push(line);
      chars += line.length + 1;
    };
    const walk = (node, depth) => {
      if (truncated) return;
      if (node.nodeType === Node.TEXT_NODE) {
        const text = clean(node.textContent, 200);
        if (text) push(depth, `text: ${text}`);
        return;
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return;
      const element = node;
      if (SKIP.has(element.nodeName) || element.hasAttribute("data-aura-annotate")) return;
      let style;
      try {
        style = getStyle(element);
      } catch {
        return;
      }
      if (hidden(element, style)) return;
      const role = roleOf(element);
      const name = role ? nameOf(element, role) : "";
      let interactive =
        (role && INTERACTIVE.has(role)) ||
        element.isContentEditable ||
        (element.hasAttribute("tabindex") && element.getAttribute("tabindex") !== "-1");
      if (!interactive && !role && clickable(element, style) && textOf(element)) {
        interactive = true;
      }
      const shown = role || interactive;
      let childDepth = depth;
      if (shown) {
        const parts = [role || "clickable"];
        if (name) parts.push(stringify(name));
        const details = [];
        if (interactive) details.push(`ref=${refFor(element)}`);
        if (role === "heading") {
          const level = element.getAttribute("aria-level") || element.localName.slice(1);
          if (/^[1-6]$/.test(level)) details.push(`level=${level}`);
        }
        if (element.disabled || element.getAttribute("aria-disabled") === "true")
          details.push("disabled");
        if (element.checked === true || element.getAttribute("aria-checked") === "true")
          details.push("checked");
        if (element.getAttribute("aria-expanded"))
          details.push(`expanded=${element.getAttribute("aria-expanded")}`);
        if (element.getAttribute("aria-selected") === "true" || element.selected === true)
          details.push("selected");
        if (
          element.getAttribute("aria-current") &&
          element.getAttribute("aria-current") !== "false"
        )
          details.push("current");
        if (details.length) parts.push(`[${details.join("] [")}]`);
        const tag = element.localName;
        if (
          (tag === "input" || tag === "textarea") &&
          element.type !== "checkbox" &&
          element.type !== "radio"
        ) {
          if (element.type === "password") {
            if (element.value) parts.push("value=(hidden)");
          } else if (element.value) parts.push(`value=${stringify(clean(element.value, 200))}`);
        } else if (tag === "select") {
          const chosen = Array.from(element.selectedOptions || [], (option) =>
            clean(option.textContent, 60),
          );
          if (chosen.length) parts.push(`value=${stringify(chosen.join(", "))}`);
        } else if (role === "link") {
          const href = element.getAttribute("href");
          if (href && !href.startsWith("javascript:")) parts.push(`-> ${clean(href, 160)}`);
        }
        push(depth, parts.join(" "));
        childDepth = depth + 1;
        // A control's own text is its name; its insides add nothing.
        if (interactive && name && role !== "listbox" && role !== "combobox" && tag !== "select")
          return;
        if (tag === "select" || tag === "textarea" || tag === "input") return;
      }
      if (element.localName === "iframe") {
        let inner = null;
        try {
          inner = element.contentDocument;
        } catch {
          inner = null;
        }
        if (inner?.body) walk(inner.body, childDepth);
        else
          push(
            childDepth,
            `(another site's frame: ${clean(element.getAttribute("src") || "", 160)})`,
          );
        return;
      }
      const root = element.shadowRoot;
      for (const child of root ? root.childNodes : element.childNodes) walk(child, childDepth);
      if (root) for (const child of element.childNodes) walk(child, childDepth);
    };
    const start = scope ? document.querySelector(scope) : document.body || document.documentElement;
    if (!start) return { text: `No element matches ${scope}.`, truncated: false };
    walk(start, 0);
    if (truncated)
      lines.push("- (the outline stops here: the page is long; scope it with a selector)");
    return { text: lines.join("\n"), truncated };
  };

  // ---------------------------------------------------------------- acting

  const targetOf = (target) => {
    if (!target) return null;
    if (/^e\d+$/.test(target)) return byRef(target);
    try {
      return document.querySelector(target);
    } catch {
      return null;
    }
  };
  const box = (element) => {
    const rect = getRect.call(element);
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  };
  const visibleIn = (rect) =>
    rect.width > 0 &&
    rect.height > 0 &&
    rect.x + rect.width > 0 &&
    rect.y + rect.height > 0 &&
    rect.x < window.innerWidth &&
    rect.y < window.innerHeight;

  // Brings the element into view and says where its centre is, and whether something else covers it.
  const locate = (target) => {
    const element = targetOf(target);
    if (!element)
      return {
        error: /^e\d+$/.test(target || "")
          ? `${target} is not on the page any more. Take a new snapshot.`
          : `Nothing matches ${target}.`,
      };
    let rect = box(element);
    if (!visibleIn(rect)) {
      element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
      rect = box(element);
    }
    const x = rect.x + rect.width / 2;
    const y = rect.y + rect.height / 2;
    let covered = null;
    const top = fromPoint(x, y).find((node) => !node.hasAttribute("data-aura-annotate"));
    if (top && top !== element && !element.contains(top) && !top.contains(element))
      covered = describeElement(top);
    return {
      x,
      y,
      rect,
      visible: rect.width > 0 && rect.height > 0,
      covered,
      disabled: element.disabled === true || element.getAttribute("aria-disabled") === "true",
      editable:
        element.isContentEditable ||
        element.localName === "textarea" ||
        (element.localName === "input" &&
          !["checkbox", "radio", "button", "submit"].includes(element.type)),
      name: clean(nameOf(element, roleOf(element)) || element.localName, 80),
    };
  };

  const fire = (element, type, init) => {
    const Kind = type.startsWith("pointer")
      ? PointerEvent
      : type.startsWith("key")
        ? KeyboardEvent
        : type === "input" || type === "beforeinput"
          ? InputEvent
          : type.startsWith("mouse") ||
              type === "click" ||
              type === "dblclick" ||
              type === "contextmenu"
            ? MouseEvent
            : Event;
    return element.dispatchEvent(
      new Kind(type, { bubbles: true, cancelable: true, composed: true, ...init }),
    );
  };
  const click = (target, options = {}) => {
    const place = locate(target);
    if (place.error) return place;
    const element = targetOf(target);
    const init = {
      clientX: place.x,
      clientY: place.y,
      button: options.button === "right" ? 2 : 0,
      detail: 1,
    };
    fire(element, "pointerover", init);
    fire(element, "pointerdown", init);
    fire(element, "mousedown", init);
    if (typeof element.focus === "function") element.focus({ preventScroll: true });
    fire(element, "pointerup", init);
    fire(element, "mouseup", init);
    if (options.button === "right") fire(element, "contextmenu", init);
    else {
      element.click();
      if (options.double) {
        element.click();
        fire(element, "dblclick", { ...init, detail: 2 });
      }
    }
    return { ok: true, name: place.name };
  };
  // React and friends watch the value through the prototype's setter.
  const setValue = (element, value) => {
    const proto = Object.getPrototypeOf(element);
    const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    if (setter) setter.call(element, value);
    else element.value = value;
  };
  const focus = (target, clear) => {
    const place = locate(target);
    if (place.error) return place;
    const element = targetOf(target);
    element.focus({ preventScroll: true });
    if (clear) {
      if (element.isContentEditable) {
        element.textContent = "";
        fire(element, "input", { inputType: "deleteContentBackward" });
      } else if ("value" in element) {
        setValue(element, "");
        fire(element, "input", { inputType: "deleteContentBackward" });
      }
    } else if ("value" in element && typeof element.setSelectionRange === "function") {
      try {
        const end = element.value.length;
        element.setSelectionRange(end, end);
      } catch {
        // Some input types have no selection.
      }
    }
    return { ok: true, name: place.name, editable: place.editable };
  };
  const type = (target, text, clear) => {
    const focused = focus(target, clear);
    if (focused.error) return focused;
    const element = targetOf(target);
    if (element.isContentEditable) {
      document.execCommand("insertText", false, text);
    } else if ("value" in element) {
      setValue(element, `${element.value}${text}`);
      fire(element, "input", { inputType: "insertText", data: text });
      fire(element, "change", {});
    }
    return { ok: true, name: focused.name };
  };
  const KEYS = {
    Enter: 13,
    Tab: 9,
    Escape: 27,
    Backspace: 8,
    Delete: 46,
    ArrowLeft: 37,
    ArrowUp: 38,
    ArrowRight: 39,
    ArrowDown: 40,
    Home: 36,
    End: 35,
    PageUp: 33,
    PageDown: 34,
    " ": 32,
  };
  const press = (key, modifiers) => {
    const element = document.activeElement || document.body;
    const init = {
      key,
      code: key.length === 1 ? `Key${key.toUpperCase()}` : key,
      keyCode: KEYS[key] ?? key.toUpperCase().charCodeAt(0),
      which: KEYS[key] ?? key.toUpperCase().charCodeAt(0),
      ctrlKey: modifiers.includes("Control"),
      shiftKey: modifiers.includes("Shift"),
      altKey: modifiers.includes("Alt"),
      metaKey: modifiers.includes("Meta"),
    };
    const go = fire(element, "keydown", init);
    if (go && key === "Enter") {
      const form = element.form || element.closest?.("form");
      if (form && element.localName !== "textarea") {
        if (typeof form.requestSubmit === "function") form.requestSubmit();
        else form.submit();
      } else if (element.localName === "a" || element.localName === "button") element.click();
    }
    if (go && key.length === 1 && !init.ctrlKey && !init.metaKey && "value" in element) {
      setValue(element, `${element.value}${key}`);
      fire(element, "input", { inputType: "insertText", data: key });
    }
    fire(element, "keyup", init);
    return { ok: true };
  };
  const select = (target, values) => {
    const place = locate(target);
    if (place.error) return place;
    const element = targetOf(target);
    if (element.localName !== "select") return { error: `${target} is not a select.` };
    const wanted = new Set(values);
    let matched = 0;
    for (const option of element.options) {
      const take = wanted.has(option.value) || wanted.has(clean(option.textContent, 200));
      option.selected = take;
      if (take) matched += 1;
    }
    fire(element, "input", {});
    fire(element, "change", {});
    return matched ? { ok: true, matched } : { error: `No option of ${target} matches.` };
  };
  const hover = (target) => {
    const place = locate(target);
    if (place.error) return place;
    const element = targetOf(target);
    const init = { clientX: place.x, clientY: place.y };
    for (const name of [
      "pointerover",
      "pointerenter",
      "mouseover",
      "mouseenter",
      "pointermove",
      "mousemove",
    ])
      fire(element, name, init);
    return { ok: true, name: place.name };
  };
  const scroll = (target, dx, dy) => {
    if (target) {
      const element = targetOf(target);
      if (!element) return { error: `${target} is not on the page any more. Take a new snapshot.` };
      if (!dx && !dy) {
        element.scrollIntoView({ block: "center", behavior: "instant" });
        return { ok: true };
      }
      element.scrollBy({ left: dx || 0, top: dy || 0, behavior: "instant" });
    } else window.scrollBy({ left: dx || 0, top: dy || 0, behavior: "instant" });
    return { ok: true, x: window.scrollX, y: window.scrollY };
  };
  const find = (text, selector) => {
    if (selector) {
      try {
        const element = document.querySelector(selector);
        return Boolean(element && box(element).width > 0);
      } catch {
        return false;
      }
    }
    // biome-ignore lint/performance/useDomNodeTextContent: text the page shows, not hidden source
    return (document.body?.innerText || "").includes(text);
  };
  const info = () => ({
    url: location.href,
    title: document.title,
    ready: document.readyState,
    scroll: { x: window.scrollX, y: window.scrollY },
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
      dpr: window.devicePixelRatio,
    },
    size: {
      width: document.documentElement.scrollWidth,
      height: document.documentElement.scrollHeight,
    },
  });

  // ---------------------------------------------------------------- Annotate

  // A short selector path that reads like the code: ids, then classes that are not generated.
  const selectorOf = (element) => {
    const parts = [];
    let node = element;
    while (node && node.nodeType === Node.ELEMENT_NODE && parts.length < 6) {
      if (node.id && !/\d{3,}|:/.test(node.id)) {
        parts.unshift(`#${CSS.escape(node.id)}`);
        break;
      }
      let part = node.localName;
      const classes = Array.from(node.classList)
        .filter((name) => !/^(css|sc|jsx|svelte|astro|emotion)-|^_|[0-9a-f]{5,}|__/.test(name))
        .slice(0, 2);
      if (classes.length) part += `.${classes.map((name) => CSS.escape(name)).join(".")}`;
      const parent = node.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter(
          (child) => child.localName === node.localName,
        );
        if (same.length > 1 && !classes.length) part += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(part);
      if (node.localName === "body" || node.localName === "main") break;
      node = node.parentElement;
    }
    return parts.join(" > ");
  };
  // The component that rendered it: React's fiber, Vue's instance or Svelte's meta.
  const componentOf = (element) => {
    try {
      for (const key of Object.keys(element)) {
        if (key.startsWith("__reactFiber$") || key.startsWith("__reactInternalInstance$")) {
          const stack = [];
          let fiber = element[key];
          while (fiber && stack.length < 8) {
            const type = fiber.type;
            const name =
              typeof type === "function"
                ? type.displayName || type.name
                : type && typeof type === "object"
                  ? type.displayName || type.render?.displayName || type.render?.name
                  : null;
            if (name && !stack.includes(name)) stack.push(name);
            fiber = fiber.return;
          }
          return stack.length
            ? { name: stack[0], stack: stack.reverse(), framework: "React" }
            : null;
        }
      }
      let vue = element.__vueParentComponent;
      if (vue) {
        const stack = [];
        while (vue && stack.length < 8) {
          const name = vue.type?.name || vue.type?.__name;
          if (name) stack.push(name);
          vue = vue.parent;
        }
        return stack.length ? { name: stack[0], stack: stack.reverse(), framework: "Vue" } : null;
      }
      const svelte = element.__svelte_meta?.loc;
      if (svelte) return { name: svelte.file, stack: [svelte.file], framework: "Svelte" };
    } catch {
      // A frozen or proxied object.
    }
    return null;
  };
  const STYLES = [
    "color",
    "background-color",
    "font-size",
    "font-family",
    "font-weight",
    "display",
    "position",
  ];
  const ATTRIBUTES = [
    "type",
    "name",
    "href",
    "src",
    "alt",
    "role",
    "aria-label",
    "placeholder",
    "title",
    "for",
    "value",
    "data-testid",
  ];
  const describeElement = (element) => ({
    tag: element.localName,
    id: element.id || null,
    classes: Array.from(element.classList).slice(0, 12),
    attributes: Object.fromEntries(
      ATTRIBUTES.filter(
        (name) => element.hasAttribute(name) && !(name === "value" && element.type === "password"),
      ).map((name) => [name, clean(element.getAttribute(name), 200)]),
    ),
    selector: selectorOf(element),
    // biome-ignore lint/performance/useDomNodeTextContent: the text as shown on the page
    text: clean(element.innerText ?? element.textContent ?? "", 200),
    rect: box(element),
    styles: (() => {
      const style = getStyle(element);
      return Object.fromEntries(STYLES.map((name) => [name, style.getPropertyValue(name)]));
    })(),
    component: componentOf(element),
    html: (() => {
      const html = element.outerHTML.replace(/\s+/g, " ");
      return html.length > 600 ? `${html.slice(0, 600)}…` : html;
    })(),
  });

  let annotate = null;
  const ANNOTATE_STYLE = `
    :host { all: initial; }
    .box { position: fixed; pointer-events: none; box-sizing: border-box; border: 2px solid var(--accent);
      background: color-mix(in srgb, var(--accent) 12%, transparent); border-radius: 3px;
      transition: left 60ms ease-out, top 60ms ease-out, width 60ms ease-out, height 60ms ease-out; }
    .area { border-style: dashed; transition: none; }
    .tag { position: fixed; pointer-events: none; max-width: 320px; padding: 2px 8px; border-radius: 999px;
      background: var(--accent); color: var(--ink); font: 600 11px/18px system-ui, sans-serif;
      white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    @media (prefers-reduced-motion: reduce) { .box { transition: none; } }
  `;
  const startAnnotate = (options) => {
    if (annotate) return true;
    const host = document.createElement("div");
    host.setAttribute("data-aura-annotate", "");
    const shadow = host.attachShadow({ mode: "closed" });
    try {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(ANNOTATE_STYLE);
      shadow.adoptedStyleSheets = [sheet];
    } catch {
      // Without constructable sheets the overlay still draws from its inline styles.
    }
    const accent = /^#[0-9a-f]{6}$|^rgb\(/i.test(options?.accent || "")
      ? options.accent
      : "#3b82f6";
    const ink = /^#[0-9a-f]{6}$|^rgb\(/i.test(options?.ink || "") ? options.ink : "#ffffff";
    host.style.setProperty("--accent", accent);
    host.style.setProperty("--ink", ink);
    const highlight = document.createElement("div");
    highlight.className = "box";
    const label = document.createElement("div");
    label.className = "tag";
    const area = document.createElement("div");
    area.className = "box area";
    for (const node of [highlight, label, area]) {
      node.style.display = "none";
      shadow.append(node);
    }
    (document.documentElement || document).append(host);
    let cursor = null;
    try {
      cursor = new CSSStyleSheet();
      cursor.replaceSync("*, *::before, *::after { cursor: crosshair !important; }");
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, cursor];
    } catch {
      cursor = null;
    }
    const state = { host, highlight, label, area, hovered: null, chain: [], down: null, cursor };
    const show = (element) => {
      state.hovered = element;
      if (!element) {
        highlight.style.display = "none";
        label.style.display = "none";
        return;
      }
      const rect = box(element);
      Object.assign(highlight.style, {
        display: "block",
        left: `${rect.x}px`,
        top: `${rect.y}px`,
        width: `${rect.width}px`,
        height: `${rect.height}px`,
      });
      const classes = Array.from(element.classList).slice(0, 2).join(".");
      label.textContent = `${element.localName}${classes ? `.${classes}` : ""}  ${Math.round(rect.width)}×${Math.round(rect.height)}`;
      Object.assign(label.style, {
        display: "block",
        left: `${Math.max(4, rect.x)}px`,
        top: `${rect.y > 26 ? rect.y - 24 : rect.y + rect.height + 4}px`,
      });
    };
    // A pick goes once the picker's own marks are off the page, so its screenshot shows the page
    // as it is; the next move of the pointer marks again.
    const pick = (message) => {
      show(null);
      area.style.display = "none";
      requestAnimationFrame(() => requestAnimationFrame(() => post(message)));
    };
    const under = (x, y) =>
      fromPoint(x, y).find(
        (node) => node !== host && !host.contains(node) && node !== document.documentElement,
      ) || null;
    const block = (event) => {
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    const listeners = {
      pointermove: (event) => {
        if (state.down) {
          const x = Math.min(state.down.x, event.clientX);
          const y = Math.min(state.down.y, event.clientY);
          const width = Math.abs(event.clientX - state.down.x);
          const height = Math.abs(event.clientY - state.down.y);
          if (width > 4 || height > 4) {
            state.down.dragged = true;
            show(null);
            Object.assign(area.style, {
              display: "block",
              left: `${x}px`,
              top: `${y}px`,
              width: `${width}px`,
              height: `${height}px`,
            });
          }
          return;
        }
        const element = under(event.clientX, event.clientY);
        state.chain = [];
        if (element !== state.hovered) show(element);
      },
      pointerdown: (event) => {
        block(event);
        if (event.button !== 0) return;
        state.down = { x: event.clientX, y: event.clientY, dragged: false };
      },
      pointerup: (event) => {
        block(event);
        const down = state.down;
        state.down = null;
        if (!down || event.button !== 0) return;
        area.style.display = "none";
        const base = {
          kind: "pick",
          url: location.href,
          title: document.title,
          viewport: info().viewport,
          add: event.shiftKey,
        };
        if (down.dragged) {
          const rect = {
            x: Math.min(down.x, event.clientX),
            y: Math.min(down.y, event.clientY),
            width: Math.abs(event.clientX - down.x),
            height: Math.abs(event.clientY - down.y),
          };
          const inside = under(rect.x + rect.width / 2, rect.y + rect.height / 2);
          pick({
            ...base,
            mode: "area",
            area: rect,
            elements: inside ? [describeElement(inside)] : [],
          });
          return;
        }
        const element = state.hovered || under(event.clientX, event.clientY);
        if (element)
          pick({ ...base, mode: "element", area: null, elements: [describeElement(element)] });
      },
      keydown: (event) => {
        if (event.key === "Escape") {
          block(event);
          post({ kind: "annotate-exit" });
          return;
        }
        if ((event.key === "ArrowUp" || event.key === "ArrowDown") && state.hovered) {
          block(event);
          if (event.key === "ArrowUp") {
            const parent = state.hovered.parentElement;
            if (parent && parent !== document.documentElement) {
              state.chain.push(state.hovered);
              show(parent);
            }
          } else if (state.chain.length) show(state.chain.pop());
          else {
            const child = state.hovered.firstElementChild;
            if (child) show(child);
          }
          return;
        }
        if (event.key === "Enter" && state.hovered) {
          block(event);
          pick({
            kind: "pick",
            url: location.href,
            title: document.title,
            viewport: info().viewport,
            add: event.shiftKey,
            mode: "element",
            area: null,
            elements: [describeElement(state.hovered)],
          });
        }
      },
      scroll: () => {
        if (state.hovered) show(state.hovered);
      },
    };
    for (const name of [
      "click",
      "mousedown",
      "mouseup",
      "dblclick",
      "auxclick",
      "contextmenu",
      "touchstart",
      "touchend",
    ])
      listeners[name] = block;
    for (const [name, listener] of Object.entries(listeners))
      window.addEventListener(name, listener, { capture: true, passive: false });
    state.listeners = listeners;
    annotate = state;
    return true;
  };
  const stopAnnotate = () => {
    if (!annotate) return false;
    for (const [name, listener] of Object.entries(annotate.listeners))
      window.removeEventListener(name, listener, { capture: true });
    annotate.host.remove();
    if (annotate.cursor)
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter(
        (sheet) => sheet !== annotate.cursor,
      );
    annotate = null;
    return true;
  };
  const pickAt = (x, y) => {
    const element = fromPoint(x, y).find(
      (node) => !node.hasAttribute("data-aura-annotate") && node !== document.documentElement,
    );
    return element ? describeElement(element) : null;
  };

  // ---------------------------------------------------------------- shortcuts and moves

  // The browser's own shortcuts belong to Ghost, as a browser's belong to it, and never reach
  // the page. Ctrl+K and the rest stay the page's.
  window.addEventListener(
    "keydown",
    (event) => {
      if (event.defaultPrevented && !(event.ctrlKey || event.metaKey)) return;
      const modifier = event.ctrlKey || event.metaKey;
      if (!modifier || event.altKey) return;
      const key = event.key.toLowerCase();
      let action = null;
      if (key === "l" && !event.shiftKey) action = "address";
      else if (key === "t" && !event.shiftKey) action = "new-tab";
      else if (key === "w" && !event.shiftKey) action = "close-tab";
      else if (key === "b" && event.shiftKey) action = "toggle";
      else if (key === "d" && event.shiftKey) action = "annotate";
      else if (key === "tab" || event.key === "Tab")
        action = event.shiftKey ? "previous-tab" : "next-tab";
      if (!action) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      post({ kind: "key", action });
    },
    true,
  );
  // A single-page app's moves change the address without a load.
  const moved = () => post({ kind: "location" });
  for (const method of ["pushState", "replaceState"]) {
    const original = history[method];
    if (typeof original !== "function") continue;
    history[method] = function (...args) {
      const result = original.apply(this, args);
      moved();
      return result;
    };
  }
  window.addEventListener("popstate", moved);
  window.addEventListener("hashchange", moved);

  const api = Object.freeze({
    version: 1,
    snapshot,
    locate,
    click,
    focus,
    type,
    press,
    select,
    hover,
    scroll,
    find,
    info,
    logs: (clear) => {
      const out = logs.slice();
      if (clear) logs.length = 0;
      return out;
    },
    network,
    annotate: (on, options) => (on ? startAnnotate(options) : stopAnnotate()),
    annotating: () => annotate !== null,
    pickAt,
    // The page's own password values, so Ghost can keep them out of what agents read.
    secrets: () => {
      const values = [];
      const collect = (root) => {
        for (const input of root.querySelectorAll("input[type=password]")) {
          if (input.value) values.push(input.value);
        }
      };
      collect(document);
      for (const frame of document.querySelectorAll("iframe")) {
        try {
          if (frame.contentDocument) collect(frame.contentDocument);
        } catch {
          // Another site's frame: not this page's to read.
        }
      }
      return values;
    },
  });
  Object.defineProperty(window, "__auraBrowser", {
    value: api,
    enumerable: false,
    configurable: false,
    writable: false,
  });
})();
