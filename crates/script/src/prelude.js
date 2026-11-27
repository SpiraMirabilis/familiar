// The scripting API, loaded into each session's JavaScript runtime before any user script.
// User scripts get `mu`, timers and `console`; the host drives everything through `__mu`.
// Native functions: __mu_post(kind, argsJson) queues an action, __mu_now() is milliseconds since
// the runtime started.
(() => {
  const nativePost = __mu_post;
  const nativeNow = __mu_now;
  const post = (kind, ...args) => nativePost(kind, JSON.stringify(args));
  const show = (value) => {
    if (typeof value === 'string') return value;
    if (value === undefined) return 'undefined';
    if (typeof value === 'function') return `[function ${value.name || 'anonymous'}]`;
    try {
      return JSON.stringify(value) ?? String(value);
    } catch {
      return String(value);
    }
  };
  const describe = (e) => (e && e.stack ? `${e.message}\n${e.stack}`.trimEnd() : String(e));
  const report = (where, e) => post('error', `${where}: ${describe(e)}`);

  // Event names are case-insensitive, so "gmcp:Char.Vitals" and "gmcp:char.vitals" match.
  const handlers = new Map();
  const listFor = (event) => handlers.get(String(event).toLowerCase()) || [];
  const emit = (event, ...args) => {
    for (const fn of listFor(event)) {
      try {
        fn(...args);
      } catch (e) {
        report(`mu.on("${event}")`, e);
      }
    }
  };

  let context = {};
  let vars = Object.create(null);

  const mu = {
    /** Sends text to the server as is. */
    send(text) {
      post('send', String(text));
    },
    /** Handles text as if you typed it: aliases and /commands apply. */
    input(text) {
      post('input', String(text));
    },
    /** Shows a line in the output. ANSI colour codes work. */
    echo(...parts) {
      post('display', parts.map(show).join(' '));
    },
    /** Shows a dimmed client message. */
    notice(...parts) {
      post('notice', parts.map(show).join(' '));
    },
    /** Speaks text aloud. */
    speak(...parts) {
      post('speak', parts.map(show).join(' '));
    },
    /** Handles a line as if the server sent it: triggers and hooks run on it. */
    receive(text) {
      post('receive', String(text));
    },
    /** Replaces what's in the input box. */
    setInput(text) {
      post('setinput', String(text));
    },
    /** Plays a sound file. */
    playSound(path) {
      post('sound', String(path));
    },
    /** Shows a line in a spawn window, creating it if needed. */
    spawn(window, text) {
      post('spawn', String(window), String(text));
    },
    /** Sends a GMCP message. `data` can be an object (sent as JSON) or a string. */
    gmcp(pkg, data) {
      const json = data === undefined ? null : typeof data === 'string' ? data : JSON.stringify(data);
      post('gmcp', String(pkg), json);
    },
    /**
     * Listens for an event: "line" (return false to hide the line), "receive", "send" (return a
     * string to change the command, or false to drop it), "gmcp" (package, data), "gmcp:Package"
     * (data), "connect" and "disconnect". Returns the handler, for mu.off.
     */
    on(event, fn) {
      if (typeof fn !== 'function') throw new TypeError('mu.on needs a function');
      const key = String(event).toLowerCase();
      handlers.set(key, [...listFor(key), fn]);
      return fn;
    },
    off(event, fn) {
      const key = String(event).toLowerCase();
      const list = listFor(key);
      const i = list.indexOf(fn);
      if (i < 0) return false;
      handlers.set(key, list.filter((_, idx) => idx !== i));
      return true;
    },
    get world() {
      return context.world;
    },
    get host() {
      return context.host;
    },
    get port() {
      return context.port;
    },
    get character() {
      return context.character ?? null;
    },
    get connected() {
      return !!context.connected;
    },
    /** Variables (the same ones as %name% and /set). Assigning saves them. */
    vars: new Proxy(
      Object.create(null),
      {
        get: (_, key) => (typeof key === 'string' && Object.hasOwn(vars, key) ? vars[key] : undefined),
        set: (_, key, value) => {
          if (typeof key !== 'string') return false;
          const str = String(value);
          vars[key] = str;
          post('setvar', key, str);
          return true;
        },
        deleteProperty: (_, key) => {
          if (typeof key !== 'string') return false;
          delete vars[key];
          post('setvar', key, null);
          return true;
        },
        has: (_, key) => typeof key === 'string' && Object.hasOwn(vars, key),
        ownKeys: () => Reflect.ownKeys(vars),
        getOwnPropertyDescriptor: (_, key) =>
          typeof key === 'string' && Object.hasOwn(vars, key)
            ? { value: vars[key], enumerable: true, configurable: true, writable: true }
            : undefined,
      },
    ),
  };

  const MAX_TIMER_MS = 2_147_483_647;
  const timers = new Map();
  let nextTimer = 1;
  const addTimer = (fn, ms, repeat, args) => {
    if (typeof fn !== 'function') throw new TypeError('timers need a function');
    const raw = Number(ms);
    const delay = Number.isFinite(raw) ? Math.min(Math.max(0, raw), MAX_TIMER_MS) : 0;
    const id = nextTimer++;
    // Intervals of at least 10 ms, so a 0 ms interval can't spin.
    timers.set(id, { fn, args, due: nativeNow() + delay, interval: repeat ? Math.max(10, delay) : null });
    return id;
  };
  globalThis.setTimeout = (fn, ms, ...args) => addTimer(fn, ms, false, args);
  globalThis.setInterval = (fn, ms, ...args) => addTimer(fn, ms, true, args);
  globalThis.clearTimeout = globalThis.clearInterval = (id) => {
    timers.delete(id);
  };

  const log = (...parts) => mu.notice(...parts);
  globalThis.console = { log, info: log, debug: log, warn: log, error: (...parts) => post('error', parts.map(show).join(' ')) };
  globalThis.mu = mu;

  const hooks = {
    show,
    describe,
    setContext(json) {
      context = JSON.parse(json);
    },
    setVars(json) {
      vars = Object.assign(Object.create(null), JSON.parse(json));
    },
    receive(text) {
      emit('receive', text);
    },
    line(text) {
      let keep = true;
      for (const fn of listFor('line')) {
        try {
          if (fn(text) === false) keep = false;
        } catch (e) {
          report('mu.on("line")', e);
        }
      }
      return keep;
    },
    send(text) {
      let command = text;
      for (const fn of listFor('send')) {
        try {
          const result = fn(command);
          if (result === false || result === null) return null;
          if (typeof result === 'string') command = result;
        } catch (e) {
          report('mu.on("send")', e);
        }
      }
      return command;
    },
    gmcp(pkg, data) {
      let parsed = null;
      try {
        parsed = data === '' ? null : JSON.parse(data);
      } catch {
        parsed = data;
      }
      emit('gmcp', pkg, parsed);
      emit(`gmcp:${pkg}`, parsed);
    },
    event(name) {
      emit(name);
    },
    call(name, argsJson) {
      if (name.startsWith('beipmu:')) {
        try {
          globalThis.__beipmu.callTrigger(name.slice(7), JSON.parse(argsJson));
        } catch (e) {
          report(name.slice(7), e);
        }
        return;
      }
      const fn = globalThis[name];
      if (typeof fn !== 'function') {
        post('error', `trigger script "${name}" isn't a function. Define it with function ${name}(line, ...groups) { ... }`);
        return;
      }
      try {
        fn(...JSON.parse(argsJson));
      } catch (e) {
        report(name, e);
      }
    },
    tick(now) {
      const due = [...timers.entries()].filter(([, t]) => t.due <= now).sort((a, b) => a[1].due - b[1].due);
      for (const [id, timer] of due) {
        if (!timers.has(id)) continue; // Cleared by an earlier timer.
        if (timer.interval === null) timers.delete(id);
        else {
          const next = timer.due + timer.interval;
          timer.due = next <= now ? now + timer.interval : next;
        }
        try {
          timer.fn(...timer.args);
        } catch (e) {
          report('timer', e);
        }
      }
    },
    nextDue() {
      let min = -1;
      for (const t of timers.values()) if (min < 0 || t.due < min) min = t.due;
      return min;
    },
  };
  Object.defineProperty(globalThis, '__mu', { value: hooks, writable: false, configurable: false, enumerable: false });
})();
