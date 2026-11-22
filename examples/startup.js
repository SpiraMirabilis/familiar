// Example startup script. Copy it to ~/.config/familiar/startup.js (it runs for every
// connection), or set it as a world's script file in the Connect dialog.
//
// The API, all on the global `mu` object:
//   mu.send(text)            send to the server as is
//   mu.input(text)           as if typed: aliases and /commands apply
//   mu.echo(...values)       show a line in the output (ANSI colours work)
//   mu.notice(...values)     show a dimmed client message (console.log does the same)
//   mu.spawn(window, text)   show a line in a spawn window
//   mu.gmcp(package, data)   send GMCP; data can be an object
//   mu.vars.name             read or assign variables (the same ones as %name% and /set)
//   mu.world, mu.host, mu.port, mu.character, mu.connected
//   mu.on(event, fn) / mu.off(event, fn)
//      "line"        fn(text): return false to hide the line
//      "receive"     fn(text): the raw line, before triggers
//      "send"        fn(command): return a string to change it, or false to drop it
//      "gmcp"        fn(package, data), or "gmcp:Char.Vitals" for one package: fn(data)
//      "connect", "disconnect"
//   setTimeout, setInterval, clearTimeout, clearInterval
//
// A trigger can also call a function here: put its name in "Call script function". It gets
// the line and then each regex group: function onTell(line, who, message) { ... }
//
// Type /@code to run JavaScript right away, e.g. /@mu.vars, and /script reload after editing.

mu.notice(`Scripts loaded for ${mu.world}${mu.character ? ` as ${mu.character}` : ''}`);

// Shortcuts: "#5 north" sends "north" five times.
mu.on('send', (command) => {
  const repeat = /^#(\d+) (.+)$/.exec(command);
  if (!repeat) return;
  for (let i = 0; i < Number(repeat[1]); i++) mu.send(repeat[2]);
  return false; // The original "#5 north" isn't sent.
});

// Keep a running tally from GMCP, if the game sends Char.Vitals.
mu.on('gmcp:Char.Vitals', (vitals) => {
  if (vitals && vitals.hp !== undefined) mu.vars.hp = vitals.hp;
});

// Hide noisy channels without a trigger.
mu.on('line', (text) => !text.startsWith('[Newbie]'));

// Called by a trigger with "Call script function" set to onTell, and a regex pattern like
// ^(\w+) tells you '(.*)'$
function onTell(line, who, message) {
  mu.spawn('Tells', `${who}: ${message}`);
}

// Say how long the session has been connected, every 30 minutes.
let minutes = 0;
setInterval(() => {
  minutes += 30;
  if (mu.connected) mu.notice(`Connected for ${minutes} minutes`);
}, 30 * 60 * 1000);
