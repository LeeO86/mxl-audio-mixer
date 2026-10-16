// A fake peer on a slave's remote link (§13), for link.sh (Node 22: global WebSocket).
//
// node link_peer.mjs silent <url>          connect and say nothing
// node link_peer.mjs big <url>             send a 5 MiB hello
// node link_peer.mjs master <url> <token>  say hello as a master
//
// Prints {"why", "closed_after_s", "messages"} when the slave closes the socket,
// or after 20 s ("still open").
const [mode, url, token] = process.argv.slice(2);
const started = Date.now();
const messages = [];
const done = (why) => {
  console.log(JSON.stringify({ why, closed_after_s: (Date.now() - started) / 1000, messages }));
  process.exit(0);
};
const ws = new WebSocket(url);
ws.onopen = () => {
  if (mode === "big") {
    ws.send(JSON.stringify({ type: "hello", role: "master", token: "x".repeat(5 << 20), label: "big" }));
  } else if (mode === "master") {
    ws.send(JSON.stringify({ type: "hello", role: "master", token, label: "second" }));
  }
};
ws.onmessage = (ev) => {
  if (messages.length < 5) messages.push(JSON.parse(ev.data));
};
ws.onclose = () => done("closed");
ws.onerror = () => {}; // onclose follows
setTimeout(() => done("still open"), 20000);
