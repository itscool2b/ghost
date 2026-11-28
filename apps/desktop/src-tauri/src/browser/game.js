// Ghost's game clock (docs/game.md, "Web games"), injected with the page agent into every top
// document of the browser beside the chat, and asleep in all of them but one: a tab the chat's
// agent armed with game_run, which says so in the page's own session storage and loads again.
// There it stands between the game and time: requestAnimationFrame, performance.now, Date and
// timers read a clock Ghost holds, so a game can be frozen while an agent thinks, played for an
// exact stretch of game time in fixed steps of 1/60 s, and photographed frame by frame.
//
// It reaches Ghost only by being called (`window.__ghostGame`), and what it answers is the page's
// own word: data, never an instruction. Frames and workers of other origins keep real time.
(() => {
  if (window.top !== window || Object.hasOwn(window, "__ghostGame")) return;

  const FLAG = "ghost.game";
  const STEP = 1000 / 60;
  let armed = false;
  try {
    armed = window.sessionStorage.getItem(FLAG) === "1";
  } catch {
    // A page without storage (an opaque origin) cannot be armed.
  }

  // The page may replace these later; keep the originals.
  const realRaf = window.requestAnimationFrame.bind(window);
  const realNow = performance.now.bind(performance);
  const RealDate = Date;
  const realSetTimeout = window.setTimeout.bind(window);
  const realClearTimeout = window.clearTimeout.bind(window);
  const realSetInterval = window.setInterval.bind(window);
  const realClearInterval = window.clearInterval.bind(window);
  const stringify = JSON.stringify;

  const clock = {
    /** The game's time, in the milliseconds performance.now() counts. */
    now: realNow(),
    frozen: false,
    speed: 1,
    /** Frames run since the page loaded. */
    frames: 0,
    /** Frames a second while live, measured over the last second. */
    fps: 0,
  };
  const dateBase = RealDate.now() - clock.now;
  const rafQueue = new Map();
  let rafId = 0;
  const timers = new Map();
  let timerId = 1_000_000;
  /** Errors a frame or a timer of the game threw while Ghost ran it. */
  const thrown = [];
  /** The action or wait under way, and the last one finished. */
  let job = null;

  const note = (error) => {
    const text = String(error?.stack || error?.message || error).slice(0, 600);
    thrown.push(text);
    if (thrown.length > 40) thrown.shift();
    try {
      console.error(error);
    } catch {
      // A console the page broke.
    }
  };

  const fireTimers = () => {
    for (let rounds = 0; rounds < 1000; rounds += 1) {
      let next = null;
      for (const timer of timers.values()) {
        if (timer.at <= clock.now && (next === null || timer.at < next.at)) next = timer;
      }
      if (next === null) return;
      if (next.every === null) timers.delete(next.id);
      else next.at += next.every;
      try {
        next.fn.apply(window, next.args);
      } catch (error) {
        note(error);
      }
    }
  };

  const frame = () => {
    clock.frames += 1;
    const callbacks = [...rafQueue.values()];
    rafQueue.clear();
    for (const callback of callbacks) {
      try {
        callback(clock.now);
      } catch (error) {
        note(error);
      }
    }
  };

  /** One fixed step of game time: the clock moves, due timers fire, the frame runs. */
  const step = () => {
    clock.now += STEP;
    fireTimers();
    frame();
  };

  const install = () => {
    window.requestAnimationFrame = (callback) => {
      rafId += 1;
      if (typeof callback === "function") rafQueue.set(rafId, callback);
      return rafId;
    };
    window.cancelAnimationFrame = (id) => {
      rafQueue.delete(id);
    };
    try {
      performance.now = () => clock.now;
    } catch {
      // A frozen performance object keeps real time; frames still step.
    }
    class GameDate extends RealDate {
      constructor(...args) {
        if (args.length === 0) super(dateBase + clock.now);
        else super(...args);
      }
      static now() {
        return Math.floor(dateBase + clock.now);
      }
    }
    window.Date = GameDate;
    const add = (fn, delay, args, every) => {
      timerId += 1;
      const wait = Math.max(0, Number(delay) || 0);
      timers.set(timerId, {
        id: timerId,
        at: clock.now + (every ? Math.max(1, wait) : wait),
        fn,
        args,
        every: every ? Math.max(1, wait) : null,
      });
      return timerId;
    };
    // A timer given code as text keeps real time: it cannot be called as a function.
    window.setTimeout = (fn, delay, ...args) =>
      typeof fn === "function" ? add(fn, delay, args, false) : realSetTimeout(fn, delay, ...args);
    window.setInterval = (fn, delay, ...args) =>
      typeof fn === "function" ? add(fn, delay, args, true) : realSetInterval(fn, delay, ...args);
    window.clearTimeout = (id) => {
      if (!timers.delete(id)) realClearTimeout(id);
    };
    window.clearInterval = (id) => {
      if (!timers.delete(id)) realClearInterval(id);
    };

    // Live: the game's clock follows real time, a frame for each of the display's.
    let last = realNow();
    let second = last;
    let counted = 0;
    const pump = (real) => {
      realRaf(pump);
      const passed = Math.min(100, Math.max(0, real - last));
      last = real;
      if (clock.frozen || job?.state === "running") return;
      clock.now += passed * clock.speed;
      fireTimers();
      frame();
      counted += 1;
      if (real - second >= 1000) {
        clock.fps = Math.round((counted * 1000) / (real - second));
        counted = 0;
        second = real;
      }
    };
    realRaf(pump);
    // A page nobody draws (its tab is not shown) gets no display frames: timers still run.
    realSetInterval(() => {
      const real = realNow();
      if (real - last < 250 || clock.frozen || job?.state === "running") return;
      const passed = Math.min(250, real - last);
      last = real;
      clock.now += passed * clock.speed;
      fireTimers();
    }, 250);

    // Three.js announces every scene and renderer to this hook when it exists before three loads.
    if (!window.__THREE_DEVTOOLS__) {
      try {
        const hook = new EventTarget();
        hook.addEventListener("observe", (event) => {
          const seen = event.detail;
          if (seen?.isScene) three.scenes.add(seen);
          else if (seen?.isWebGLRenderer || seen?.domElement) three.renderers.add(seen);
        });
        window.__THREE_DEVTOOLS__ = hook;
      } catch {
        // No EventTarget constructor: the outline falls back to what the page shows.
      }
    }
  };

  const three = { scenes: new Set(), renderers: new Set() };

  // ---------------------------------------------------------------- pictures

  /** The game's canvas: the largest one on the page that shows. */
  const gameCanvas = () => {
    let best = null;
    let area = 0;
    for (const canvas of document.querySelectorAll("canvas")) {
      const rect = canvas.getBoundingClientRect();
      const size = rect.width * rect.height;
      if (size > area && canvas.width > 0 && canvas.height > 0) {
        best = canvas;
        area = size;
      }
    }
    return best;
  };

  /** The canvas as it is now, no wider or taller than `max`, on a scratch canvas. */
  const grab = (max) => {
    const canvas = gameCanvas();
    if (!canvas) return null;
    const scale = Math.min(1, max / Math.max(canvas.width, canvas.height));
    const copy = document.createElement("canvas");
    copy.width = Math.max(1, Math.round(canvas.width * scale));
    copy.height = Math.max(1, Math.round(canvas.height * scale));
    const context = copy.getContext("2d");
    if (!context) return null;
    try {
      context.drawImage(canvas, 0, 0, copy.width, copy.height);
    } catch {
      return null;
    }
    return copy;
  };

  const jpeg = (canvas, quality) => {
    if (!canvas) return null;
    try {
      const url = canvas.toDataURL("image/jpeg", quality);
      const at = url.indexOf(",");
      return at < 0
        ? null
        : { data: url.slice(at + 1), width: canvas.width, height: canvas.height };
    } catch {
      // A canvas another site's picture tainted cannot be read.
      return null;
    }
  };

  /** Frames side by side, left to right and top to bottom, as one picture. */
  const strip = (frames) => {
    if (frames.length < 2) return null;
    const cell = frames[0];
    const columns = Math.min(frames.length, 4);
    const rows = Math.ceil(frames.length / columns);
    const gap = 4;
    const sheet = document.createElement("canvas");
    sheet.width = columns * cell.width + (columns - 1) * gap;
    sheet.height = rows * cell.height + (rows - 1) * gap;
    const context = sheet.getContext("2d");
    if (!context) return null;
    context.fillStyle = "#121212";
    context.fillRect(0, 0, sheet.width, sheet.height);
    frames.forEach((frame, index) => {
      context.drawImage(
        frame,
        (index % columns) * (cell.width + gap),
        Math.floor(index / columns) * (cell.height + gap),
      );
    });
    return jpeg(sheet, 0.8);
  };

  // ---------------------------------------------------------------- input

  const NAMED = {
    ArrowLeft: 37,
    ArrowUp: 38,
    ArrowRight: 39,
    ArrowDown: 40,
    Space: 32,
    Enter: 13,
    Escape: 27,
    Tab: 9,
    Backspace: 8,
    ShiftLeft: 16,
    ControlLeft: 17,
    AltLeft: 18,
  };
  // The names Godot's own actions go by, so one habit serves both kinds of game.
  const ALIASES = {
    ui_left: "ArrowLeft",
    ui_right: "ArrowRight",
    ui_up: "ArrowUp",
    ui_down: "ArrowDown",
    ui_accept: "Enter",
    ui_select: "Space",
    ui_cancel: "Escape",
    Left: "ArrowLeft",
    Right: "ArrowRight",
    Up: "ArrowUp",
    Down: "ArrowDown",
    Shift: "ShiftLeft",
    Control: "ControlLeft",
    Alt: "AltLeft",
    " ": "Space",
  };

  /** A key by its name: its `code`, the `key` it types and its old numeric code. */
  const keyOf = (name) => {
    const code = ALIASES[name] ?? name;
    if (Object.hasOwn(NAMED, code)) {
      const key = code === "Space" ? " " : code.endsWith("Left") ? code.slice(0, -4) : code;
      return { code, key, keyCode: NAMED[code] };
    }
    let match = /^Key([A-Z])$/.exec(code);
    if (match) return { code, key: match[1].toLowerCase(), keyCode: match[1].charCodeAt(0) };
    match = /^Digit([0-9])$/.exec(code);
    if (match) return { code, key: match[1], keyCode: match[1].charCodeAt(0) };
    match = /^F([1-9]|1[0-2])$/.exec(code);
    if (match) return { code, key: code, keyCode: 111 + Number(match[1]) };
    if (/^[a-zA-Z]$/.test(code)) {
      const upper = code.toUpperCase();
      return { code: `Key${upper}`, key: code.toLowerCase(), keyCode: upper.charCodeAt(0) };
    }
    if (/^[0-9]$/.test(code))
      return { code: `Digit${code}`, key: code, keyCode: code.charCodeAt(0) };
    return null;
  };

  const sendKey = (type, key) => {
    const event = new KeyboardEvent(type, {
      key: key.key,
      code: key.code,
      bubbles: true,
      cancelable: true,
      composed: true,
    });
    // Older game code reads the numeric codes, which the constructor does not take everywhere.
    for (const name of ["keyCode", "which"]) {
      try {
        Object.defineProperty(event, name, { get: () => key.keyCode });
      } catch {
        // An engine that forbids it keeps zero.
      }
    }
    const target =
      document.activeElement && document.activeElement !== document.body
        ? document.activeElement
        : (document.body ?? document.documentElement);
    target.dispatchEvent(event);
  };

  /** A point of the picture as a point of the page, and what is there. */
  const pointOf = (at) => {
    const canvas = gameCanvas();
    if (!canvas || !Array.isArray(at) || at.length !== 2) return null;
    const rect = canvas.getBoundingClientRect();
    const x = rect.left + (Number(at[0]) / canvas.width) * rect.width;
    const y = rect.top + (Number(at[1]) / canvas.height) * rect.height;
    if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
    return { x, y, target: document.elementFromPoint(x, y) ?? canvas };
  };

  const BUTTONS = { left: 0, middle: 1, right: 2 };

  const sendMouse = (type, at, button) => {
    const point = pointOf(at);
    if (!point) return;
    const init = {
      clientX: point.x,
      clientY: point.y,
      button: BUTTONS[button] ?? 0,
      buttons: type.endsWith("down") ? 1 : 0,
      bubbles: true,
      cancelable: true,
      composed: true,
      pointerId: 1,
      pointerType: "mouse",
      isPrimary: true,
    };
    if (type === "move") {
      point.target.dispatchEvent(new PointerEvent("pointermove", init));
      point.target.dispatchEvent(new MouseEvent("mousemove", init));
      return;
    }
    point.target.dispatchEvent(new PointerEvent(`pointer${type}`, init));
    point.target.dispatchEvent(new MouseEvent(`mouse${type}`, init));
    if (type === "up") point.target.dispatchEvent(new MouseEvent("click", init));
  };

  // ---------------------------------------------------------------- jobs

  const yieldTask = () =>
    new Promise((resolve) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = () => resolve();
      channel.port2.postMessage(0);
    });

  /** What an action's steps mean, or why they cannot be played. */
  const plan = (steps) => {
    if (!Array.isArray(steps) || steps.length === 0 || steps.length > 64) {
      return { error: "An action is 1 to 64 steps." };
    }
    const out = [];
    let total = 0;
    for (const raw of steps) {
      if (typeof raw !== "object" || raw === null) return { error: "A step is an object." };
      const names = [];
      if (typeof raw.press === "string") names.push(raw.press);
      if (Array.isArray(raw.hold)) names.push(...raw.hold.map(String));
      const keys = [];
      for (const name of names) {
        const key = keyOf(name);
        if (!key) {
          return {
            error: `A web game has no key "${String(name).slice(0, 40)}". Use a key's name: ArrowLeft, Space, KeyW, Enter.`,
          };
        }
        keys.push(key);
      }
      const frames = Math.max(1, Math.round((Number(raw.ms) || 100) / STEP));
      total += frames;
      out.push({
        keys,
        frames,
        click: Array.isArray(raw.click) ? raw.click : null,
        move: Array.isArray(raw.move) ? raw.move : null,
        button: typeof raw.button === "string" ? raw.button : "left",
      });
    }
    if (total > 120 * 60)
      return { error: "The action is longer than two minutes of game time. Split it up." };
    return { steps: out, total };
  };

  const finish = (work, max, extra) => {
    clock.frozen = true;
    const last = jpeg(grab(max), 0.9);
    Object.assign(work, extra, {
      state: "done",
      final: last,
      game_ms: work.done * STEP,
      frames: work.done,
      errors: thrown.splice(0, thrown.length).slice(-5),
    });
  };

  const runAct = async (work, planned, shots, max) => {
    const every = Math.max(1, Math.ceil(planned.total / Math.max(1, shots)));
    const cells = [];
    for (const part of planned.steps) {
      if (part.move) sendMouse("move", part.move, part.button);
      if (part.click) {
        sendMouse("move", part.click, part.button);
        sendMouse("down", part.click, part.button);
      }
      for (const key of part.keys) sendKey("keydown", key);
      for (let count = 0; count < part.frames; count += 1) {
        step();
        work.done += 1;
        // The picture is read in the frame's own task: a WebGL canvas is gone after it.
        if (shots > 0 && work.done % every === 0 && cells.length < 12) {
          const cell = grab(360);
          if (cell) cells.push(cell);
        }
        // Promises of the game's own settle between frames, and now and then the page's other
        // work (a loaded file, a message) gets its turn.
        await Promise.resolve();
        if (work.done % 6 === 0) await yieldTask();
        if (work.cancelled) break;
      }
      for (const key of part.keys) sendKey("keyup", key);
      if (part.click) sendMouse("up", part.click, part.button);
      if (work.cancelled) break;
    }
    finish(work, max, { strip: strip(cells), strip_frames: cells.length > 1 ? cells.length : 0 });
  };

  const runWait = async (work, test, frames, max) => {
    let met = false;
    let failed = null;
    for (let count = 0; count < frames; count += 1) {
      step();
      work.done += 1;
      try {
        if (test()) {
          met = true;
          break;
        }
      } catch (error) {
        failed = String(error?.message || error).slice(0, 300);
        break;
      }
      await Promise.resolve();
      if (work.done % 6 === 0) await yieldTask();
      if (work.cancelled) break;
    }
    finish(work, max, { met, failed });
  };

  // ---------------------------------------------------------------- the scene

  /** What an agent may read of a value: short, and never an object's insides. */
  const shown = (value, limit = 240) => {
    let text;
    if (typeof value === "string") text = value;
    else if (value === undefined) text = "undefined";
    else if (typeof value === "function") text = `[function ${value.name || "anonymous"}]`;
    else {
      try {
        const seen = new WeakSet();
        text = stringify(value, (_key, item) => {
          if (typeof item === "object" && item !== null) {
            if (seen.has(item)) return "[circular]";
            seen.add(item);
          }
          if (typeof item === "bigint") return String(item);
          return item;
        });
      } catch {
        text = String(value);
      }
      if (text === undefined) text = String(value);
    }
    return text.length > limit ? `${text.slice(0, limit)}…` : text;
  };

  const round = (number) => Math.round(Number(number) * 100) / 100;
  let refs = new Map();

  const phaserGame = () => {
    const kept = globalThis.__GHOST_GAME__;
    if (kept?.scene?.getScenes) return kept;
    const games = globalThis.Phaser?.GAMES;
    return Array.isArray(games) && games.length > 0 ? games[0] : null;
  };

  const engine = () => {
    if (three.scenes.size > 0) return "three";
    if (phaserGame()) return "phaser";
    return null;
  };

  const tree = () => {
    refs = new Map();
    const nodes = [];
    let count = 0;
    const add = (object, depth, entry) => {
      count += 1;
      const ref = `w${count}`;
      refs.set(ref, object);
      nodes.push({ ref, depth, ...entry });
    };
    const LIMIT = 400;
    if (three.scenes.size > 0) {
      const walk = (object, depth) => {
        if (count >= LIMIT) return;
        const at = object.position;
        add(object, depth, {
          name: String(object.name || object.type || "Object3D").slice(0, 60),
          kind: String(object.type || "Object3D").slice(0, 40),
          at: at ? [round(at.x), round(at.y), round(at.z)] : null,
          visible: object.visible !== false,
        });
        for (const child of object.children ?? []) walk(child, depth + 1);
      };
      for (const scene of three.scenes) walk(scene, 0);
    } else {
      const game = phaserGame();
      if (game) {
        for (const scene of game.scene.getScenes(true)) {
          if (count >= LIMIT) break;
          add(scene, 0, {
            name: String(scene.sys?.settings?.key ?? "Scene").slice(0, 60),
            kind: "Scene",
            at: null,
            visible: true,
          });
          for (const child of scene.children?.list ?? []) {
            if (count >= LIMIT) break;
            add(child, 1, {
              name: String(child.name || child.type || "GameObject").slice(0, 60),
              kind: String(child.type || "GameObject").slice(0, 40),
              at: [round(child.x), round(child.y)],
              visible: child.visible !== false,
              text: typeof child.text === "string" ? child.text.slice(0, 80) : undefined,
            });
          }
        }
      }
    }
    const canvas = gameCanvas();
    return {
      engine: engine(),
      nodes,
      cut: count >= LIMIT,
      canvas: canvas ? [canvas.width, canvas.height] : null,
      title: document.title.slice(0, 120),
    };
  };

  const inspect = (ref) => {
    const object = refs.get(String(ref));
    if (!object) return { error: "No such thing now. Read the outline again with game_tree." };
    const values = {};
    let count = 0;
    for (const key of Object.keys(object)) {
      if (count >= 60) break;
      if (key.startsWith("_")) continue;
      let value;
      try {
        value = object[key];
      } catch {
        continue;
      }
      const kind = typeof value;
      if (kind === "function") continue;
      if (kind === "object" && value !== null) {
        // A vector, a colour, a small plain object: shown; anything larger is named.
        const plain = ["x", "y", "z", "w", "r", "g", "b", "width", "height"].filter(
          (part) => typeof value[part] === "number",
        );
        if (plain.length > 0) {
          values[key] = plain.map((part) => `${part} ${round(value[part])}`).join(", ");
          count += 1;
        }
        continue;
      }
      values[key] = shown(value, 120);
      count += 1;
    }
    return {
      kind: String(object.type || object.constructor?.name || "Object").slice(0, 60),
      name: String(object.name || "").slice(0, 80),
      values,
    };
  };

  // ---------------------------------------------------------------- what Ghost calls

  let jobs = 0;
  const begin = () => {
    jobs += 1;
    job = { id: jobs, state: "running", done: 0, cancelled: false };
    return job;
  };

  const api = Object.freeze({
    version: 1,
    armed: () => armed,
    /** Marks this tab's page as a game for its next load, or takes the mark away. */
    arm: (on) => {
      try {
        if (on) window.sessionStorage.setItem(FLAG, "1");
        else window.sessionStorage.removeItem(FLAG);
        return true;
      } catch {
        return false;
      }
    },
    status: () => {
      const canvas = gameCanvas();
      return {
        armed,
        frozen: clock.frozen,
        speed: clock.speed,
        fps: clock.frozen ? 0 : clock.fps,
        frames: clock.frames,
        engine: engine(),
        canvas: canvas ? [canvas.width, canvas.height] : null,
        busy: job?.state === "running",
      };
    },
    time: (frozen, speed) => {
      if (typeof frozen === "boolean") clock.frozen = frozen;
      if (typeof speed === "number" && Number.isFinite(speed)) {
        clock.speed = Math.min(4, Math.max(0.1, speed));
      }
      return { frozen: clock.frozen, speed: clock.speed };
    },
    look: (max) => {
      // A frame's picture is gone once the display has shown it: draw one more to read it.
      // While frozen that frame moves no time.
      if (armed && job?.state !== "running") frame();
      return jpeg(grab(Number(max) || 1568), 0.9);
    },
    act: (steps, shots, max) => {
      if (!armed) return { error: "not armed" };
      if (job?.state === "running")
        return { error: "The game is busy with another action. Wait for it to finish." };
      const planned = plan(steps);
      if (planned.error) return { error: planned.error };
      const work = begin();
      const count = Math.min(12, Math.max(0, Number(shots) || 0));
      runAct(work, planned, count, Number(max) || 1568).catch((error) => {
        work.state = "done";
        work.error = String(error?.message || error).slice(0, 300);
        clock.frozen = true;
      });
      return { id: work.id };
    },
    wait: (until, timeoutMs, max) => {
      if (!armed) return { error: "not armed" };
      if (job?.state === "running")
        return { error: "The game is busy with another action. Wait for it to finish." };
      // The condition is the agent's own JavaScript for this page, as game_eval's code and
      // browser_evaluate's are: running it is the tool, and it reaches nothing those do not.
      let test;
      try {
        test = new Function(`return (${String(until)});`);
      } catch (error) {
        return { error: `The condition could not be read: ${String(error.message).slice(0, 200)}` };
      }
      const frames = Math.round(Math.min(120000, Math.max(16, Number(timeoutMs) || 5000)) / STEP);
      const work = begin();
      runWait(work, test, frames, Number(max) || 1568).catch((error) => {
        work.state = "done";
        work.error = String(error?.message || error).slice(0, 300);
        clock.frozen = true;
      });
      return { id: work.id };
    },
    /** The job's answer once it is done; `null` while it runs. */
    poll: (id) => {
      if (!job || job.id !== id) return { error: "That action is gone: the page loaded again." };
      if (job.state === "running") return null;
      const { cancelled: _cancelled, ...answer } = job;
      return answer;
    },
    tree,
    inspect,
    eval: (code) => {
      // The agent's own JavaScript for this page (the same power browser_evaluate gives it in
      // this tab): one expression, or the body of a function that may return a value. It runs
      // in the page's global scope, as the page's own code does.
      let run;
      try {
        run = new Function(`return (${String(code)}\n);`);
      } catch {
        run = new Function(String(code));
      }
      const value = run.call(window);
      return { value: shown(value, 2000), errors: thrown.splice(0, thrown.length).slice(-5) };
    },
    perf: (frames) => {
      if (!armed) return { error: "not armed" };
      if (job?.state === "running") return { error: "The game is busy with another action." };
      const count = Math.min(600, Math.max(10, Number(frames) || 120));
      const was = clock.frozen;
      let worst = 0;
      const started = realNow();
      for (let index = 0; index < count; index += 1) {
        const before = realNow();
        step();
        worst = Math.max(worst, realNow() - before);
      }
      const average = (realNow() - started) / count;
      clock.frozen = was;
      return {
        frames: count,
        frame_ms: average,
        worst_ms: worst,
        could_run_at_fps: 1000 / Math.max(average, 0.001),
        canvases: document.querySelectorAll("canvas").length,
        nodes: document.getElementsByTagName("*").length,
        memory_mb: performance.memory ? performance.memory.usedJSHeapSize / (1024 * 1024) : null,
      };
    },
  });
  Object.defineProperty(window, "__ghostGame", {
    value: api,
    enumerable: false,
    configurable: false,
    writable: false,
  });
  if (armed) install();
})();
