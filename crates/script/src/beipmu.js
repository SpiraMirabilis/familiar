// BeipMU compatibility: the `app` and `window` objects BeipMU scripts use, built on `mu`. Loaded
// after prelude.js. What BeipMU could do that this client can't (ActiveX, sockets, drawing
// windows) throws an error that says so; calls that only changed BeipMU's own UI do nothing.
(() => {
  const unsupported = (what) => () => {
    throw new Error(`BeipMU's ${what} isn't available in this client`);
  };
  const warned = new Set();
  const warnOnce = (what) => () => {
    if (!warned.has(what)) {
      warned.add(what);
      mu.notice(`BeipMU script compatibility: ${what} isn't supported, so it's ignored`);
    }
  };
  const stripHtml = (html) =>
    String(html)
      .replace(/<br\s*\/?>/gi, '\n')
      .replace(/<[^>]*>/g, '')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&#39;/g, "'")
      .replace(/&nbsp;/g, ' ')
      .replace(/&amp;/g, '&');
  const escapeHtml = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

  // BeipMU's objects came through COM, so member names ignore case: window.Connection,
  // window.connection and window.CONNECTION are all the same.
  const loose = (target) =>
    new Proxy(target, {
      get(t, key, receiver) {
        if (typeof key !== 'string' || key in t) return Reflect.get(t, key, receiver);
        const lower = key.toLowerCase();
        return lower in t ? Reflect.get(t, lower, receiver) : undefined;
      },
      set(t, key, value, receiver) {
        const name = typeof key === 'string' && !(key in t) && key.toLowerCase() in t ? key.toLowerCase() : key;
        return Reflect.set(t, name, value, receiver);
      },
      has(t, key) {
        return key in t || (typeof key === 'string' && key.toLowerCase() in t);
      },
    });

  // A COM collection: callable (worlds(0)), with item(i) and count.
  const collection = (items) => {
    const list = () => (typeof items === 'function' ? items() : items);
    const pick = (i) => list()[i];
    const f = function (i) {
      return pick(i);
    };
    f.item = pick;
    Object.defineProperty(f, 'count', { get: () => list().length });
    return loose(f);
  };

  const textLine = (text) => {
    const styling = warnOnce('styling lines from scripts');
    return loose({
      get string() {
        return text;
      },
      get length() {
        return text.length;
      },
      get htmlstring() {
        return escapeHtml(text);
      },
      toString() {
        return text;
      },
      insert: styling,
      delete: styling,
      color: styling,
      bgcolor: styling,
      bold: styling,
      italic: styling,
      underline: styling,
      strikeout: styling,
      flash: styling,
      flashmode: styling,
      blink: styling,
    });
  };

  // One hook of each kind per window, as in BeipMU: setting one replaces the last.
  const hooks = {};
  const setHook = (name) => (fn, userdata) => {
    hooks[name] = typeof fn === 'function' ? { fn, userdata } : null;
  };
  const runHook = (name, ...args) => {
    const hook = hooks[name];
    return hook ? hook.fn(...args, hook.userdata) : undefined;
  };

  mu.on('line', (text) => {
    // OnReceive and OnDisplay return true when they've dealt with the line, which hides it.
    if (runHook('receive', text) === true) return false;
    if (runHook('display', textLine(text)) === true) return false;
    return true;
  });
  mu.on('send', (text) => (runHook('send', text) === true ? false : undefined));
  mu.on('gmcp', (pkg, data) => {
    const json = data === null || data === undefined ? '' : typeof data === 'string' ? data : JSON.stringify(data);
    runHook('gmcp', json ? `${pkg} ${json}` : pkg);
  });
  mu.on('connect', () => runHook('connect'));
  mu.on('disconnect', () => runHook('disconnect'));

  const makeTextWindow = (initialTitle) => {
    let title = initialTitle;
    const write = (text) => (title ? mu.spawn(title, String(text)) : mu.echo(String(text)));
    return loose({
      get properties() {
        return loose({
          get title() {
            return title ?? '';
          },
          set title(value) {
            if (initialTitle !== undefined) title = String(value);
          },
          hwnd: 0,
        });
      },
      paused: false,
      create: (text) => textLine(String(text)),
      createhtml: (html) => textLine(stripHtml(html)),
      add: (line) => write(line && line.string !== undefined ? line.string : line),
      write,
      writehtml: (html) => write(stripHtml(html)),
      setonpause: () => {},
    });
  };
  const output = makeTextWindow(undefined);
  let spawnCount = 0;

  const input = loose({
    set: (text) => mu.setInput(String(text)),
    // The input box isn't readable from scripts here.
    get: () => '',
    length: 0,
    prefix: '',
    title: '',
    setsel: () => {},
    getselstart: () => 0,
    getselend: () => 0,
  });

  const world = () =>
    mu.world === undefined
      ? null
      : loose({
          get name() {
            return mu.world;
          },
          get host() {
            return `${mu.host}:${mu.port}`;
          },
          info: '',
        });
  const character = () => (mu.character === null ? null : loose({ name: mu.character, info: '' }));

  let win;
  const connection = loose({
    send: (text) => mu.send(String(text)),
    transmit: (text) => mu.send(String(text)),
    receive: (text) => mu.receive(String(text)),
    display: (text) => mu.echo(String(text)),
    sendgmcp: (pkg, json) => mu.gmcp(String(pkg), json === undefined ? undefined : String(json)),
    setonsend: setHook('send'),
    setonreceive: setHook('receive'),
    setondisplay: setHook('display'),
    setongmcp: setHook('gmcp'),
    setonconnect: setHook('connect'),
    setondisconnect: setHook('disconnect'),
    isconnected: () => mu.connected,
    reconnect: () => {
      if (mu.connected) return false;
      mu.input('/reconnect');
      return true;
    },
    islogging: () => false,
    log: loose({ write: warnOnce('writing to the log'), writeline: warnOnce('writing to the log'), filename: '' }),
    get world() {
      return world();
    },
    get character() {
      return character();
    },
    puppet: null,
    get window_main() {
      return win;
    },
  });

  win = loose({
    output,
    history: output,
    input,
    connection,
    userdata: undefined,
    getinput: () => input,
    getspawntabs: () => loose({ setontabactivate: () => {} }),
    makedocking: () => loose({ dock: () => {} }),
    close: warnOnce('window.Close'),
    setoncommand: warnOnce('window.SetOnCommand'),
    setonactivate: () => {},
    setonclose: () => {},
    run: (command) => mu.input(String(command)),
    runfile: (path) => mu.input(`/script load ${path}`),
    createdialogconnect: () => {},
    titleprefix: '',
    get title() {
      return mu.character ? `${mu.character} @ ${mu.world}` : (mu.world ?? '');
    },
    activity: () => {},
    addimportantactivity: () => {},
    getvariable: (name) => mu.vars[name] ?? '',
    setvariable: (name, value) => {
      mu.vars[name] = value;
    },
    deletevariable: (name) => {
      delete mu.vars[name];
    },
  });

  const timer = (ms, fn, userdata, repeat) => {
    if (typeof fn !== 'function') throw new TypeError('timers need a function');
    let active = true;
    let id;
    const t = loose({
      userdata,
      get active() {
        return active;
      },
      kill() {
        active = false;
        clearTimeout(id);
      },
    });
    id = (repeat ? setInterval : setTimeout)(() => {
      if (!repeat) active = false;
      // Returning true from the callback stops the timer.
      if (fn(t.userdata) === true) t.kill();
    }, ms);
    return t;
  };

  const app = loose({
    buildnumber: 0,
    builddate: new Date(0),
    version: 0,
    get worlds() {
      return collection(() => [world()].filter(Boolean));
    },
    windows: collection([win]),
    triggers: collection([]),
    aliases: collection([]),
    configpath: '',
    activexobject: unsupported('ActiveXObject'),
    newtrigger: unsupported('NewTrigger'),
    newwindow: unsupported('NewWindow'),
    newwindow_fixedtext: unsupported('NewWindow_FixedText'),
    newwindow_graphics: unsupported('NewWindow_Graphics'),
    // A text window becomes a spawn window, which docks beside the output.
    newwindow_text: () => makeTextWindow(`Script ${++spawnCount}`),
    setonnewwindow: () => {},
    createinterval: (ms, fn, userdata) => timer(ms, fn, userdata, true),
    createtimeout: (ms, fn, userdata) => timer(ms, fn, userdata, false),
    new_socket: unsupported('New_Socket'),
    new_socketserver: unsupported('New_SocketServer'),
    reversednslookup: unsupported('ReverseDNSLookup'),
    forwarddnslookup: unsupported('ForwardDNSLookup'),
    isaddress: (text) => /^(\d{1,3}\.){3}\d{1,3}$|^[0-9a-f:]+:[0-9a-f:]*$/i.test(String(text)),
    playsound: (file) => mu.playSound(String(file)),
    stopsounds: () => {},
    outputdebughtml: (html) => mu.notice(stripHtml(html)),
    outputdebugtext: (text) => mu.notice(String(text)),
  });

  globalThis.app = app;
  globalThis.window = win;

  // Trigger "beipmu:Name" script actions: BeipMU called Name(ranges, line, window), where
  // ranges holds the start and end of the match and of each group.
  globalThis.__beipmu = {
    callTrigger(name, args) {
      const fn = globalThis[name];
      if (typeof fn !== 'function') throw new Error(`trigger script "${name}" isn't a function`);
      const [text, ...offsets] = args;
      const ranges = offsets.map(Number);
      fn(collection(ranges), textLine(text), win);
    },
  };
})();
