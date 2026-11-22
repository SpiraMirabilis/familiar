// Injected into every web view before the page's own scripts run. Pages reach the client as
// `window.mu.client`, or as `window.chrome.webview.hostObjects.client` for pages written for
// BeipMU. Method names are case-insensitive, as they were in BeipMU.
(() => {
  if (window.mu) return;
  const post = (message) => window.webkit.messageHandlers.mu.postMessage(message);
  const listeners = { send: null, receive: null, gmcp: [], display: new Map() };

  const methods = {
    Send(text, processAliases = false) {
      post({ op: 'send', text: String(text), aliases: !!processAliases });
    },
    // BeipMU's docs call this both as SendGMCP(package, json) and as SendGMCP("Package json").
    SendGMCP(pkg, json) {
      let name = String(pkg);
      let data = json === undefined ? null : String(json);
      if (data === null) {
        const space = name.indexOf(' ');
        if (space > 0) {
          data = name.slice(space + 1);
          name = name.slice(0, space);
        }
      }
      post({ op: 'gmcp', package: name, data });
    },
    Receive(text) {
      post({ op: 'receive', text: String(text) });
    },
    Display(text) {
      post({ op: 'display', text: String(text) });
    },
    CloseWindow() {
      post({ op: 'close' });
    },
    SetOnSend(callback) {
      listeners.send = callback;
    },
    SetOnReceive(callback) {
      listeners.receive = callback;
    },
    SetOnGMCP(callback, packagePrefix = '') {
      listeners.gmcp.push({ callback, prefix: String(packagePrefix).toLowerCase() });
    },
    SetOnDisplay(id, callback, regex) {
      listeners.display.set(id, { callback, regex: new RegExp(regex) });
    },
    ClearOnDisplay(id) {
      return listeners.display.delete(id);
    },
  };

  const lookup = (target, name) => {
    if (typeof name !== 'string') return undefined;
    if (name in target) return target[name];
    const key = Object.keys(target).find((k) => k.toLowerCase() === name.toLowerCase());
    return key === undefined ? undefined : target[key];
  };
  const client = new Proxy(methods, { get: lookup });

  // Called by the client with events: gmcp, receive, send and display.
  const dispatch = (event) => {
    try {
      switch (event.type) {
        case 'gmcp':
          for (const l of listeners.gmcp) {
            if (event.package.toLowerCase().startsWith(l.prefix)) l.callback(event.package, event.data);
          }
          break;
        case 'receive':
          if (listeners.receive) listeners.receive(event.text);
          break;
        case 'send':
          if (listeners.send) listeners.send(event.text);
          break;
        case 'display':
          for (const [id, l] of listeners.display) {
            const match = l.regex.exec(event.text);
            // BeipMU passed a TextWindowLine; pages use its `string` and `length`.
            if (match) l.callback(id, { string: event.text, length: event.text.length }, match);
          }
          break;
      }
    } catch (e) {
      console.error('MU Client web view callback failed:', e);
    }
  };

  window.mu = { client, _dispatch: dispatch };
  window.chrome = window.chrome || {};
  window.chrome.webview = window.chrome.webview || {};
  window.chrome.webview.hostObjects = { client, options: {} };
})();
