// A TLS-terminating proxy, for link.sh's wss:// case: what an ingress does in
// front of a slave's web port.
//
// node tls_proxy.mjs <listen port> <cert.pem> <key.pem> <target port on 127.0.0.1>
import { readFileSync } from "node:fs";
import net from "node:net";
import tls from "node:tls";

const [port, cert, key, target] = process.argv.slice(2);
tls
  .createServer({ cert: readFileSync(cert), key: readFileSync(key) }, (client) => {
    const upstream = net.connect(Number(target), "127.0.0.1");
    client.pipe(upstream).pipe(client);
    client.on("error", () => upstream.destroy());
    upstream.on("error", () => client.destroy());
  })
  .listen(Number(port), "127.0.0.1");
