// REST client for the mixer API (README "API") and display helpers.

async function request(path, options) {
  return (await exchange(path, options)).data;
}

/** The answer and the mix revision after a change (`x-mix-revision`). */
export async function exchange(path, { method = "GET", body, text = false } = {}) {
  const headers = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const resp = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
    cache: "no-store",
    // A hung request ends after 30 s, so a poll waiting for it carries on.
    signal: AbortSignal.timeout(30000),
  });
  const raw = await resp.text();
  let data = raw;
  if (!text) {
    try {
      data = raw ? JSON.parse(raw) : null;
    } catch {
      data = raw;
    }
  }
  if (!resp.ok) {
    const reason = data && typeof data === "object" ? data.error : String(data || "").slice(0, 200);
    // The answer stays on the error: a refused logic edit lists its issues there (D162).
    throw Object.assign(new Error(reason || `HTTP ${resp.status}`), { status: resp.status, data });
  }
  const revision = resp.headers.get("x-mix-revision");
  return { data, revision: revision === null ? null : Number(revision) };
}

export const api = {
  get: (path) => request(path),
  text: (path) => request(path, { text: true }),
  post: (path, body) => request(path, { method: "POST", body: body ?? {} }),
  put: (path, body) => request(path, { method: "PUT", body: body ?? {} }),
  patch: (path, body) => request(path, { method: "PATCH", body }),
  del: (path) => request(path, { method: "DELETE" }),
};

/** HTTP status of a probe (/livez, /readyz) without throwing; 0 when it does not answer. */
export async function probe(path) {
  try {
    return (await fetch(path, { cache: "no-store" })).status;
  } catch {
    return 0;
  }
}

/** The first 8 characters of an id; the whole id goes into a title. */
export const shortId = (id) => (id ? String(id).slice(0, 8) : "–");

/** Copies text; falls back to a hidden textarea on plain http, where navigator.clipboard is missing. */
export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  }
}

export function download(name, text, type = "application/json") {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** A gain from the API: a number of dB or "-inf". */
export const dbValue = (v) => (v === "-inf" || v === null || v === undefined ? -Infinity : Number(v));

/** dB with one decimal and a real minus; −∞ for silence. */
export function fmtDb(v, digits = 1) {
  const x = dbValue(v);
  if (!Number.isFinite(x) || x <= -199) return "−∞";
  const t = Math.abs(x).toFixed(digits);
  return `${x < 0 && Number(t) !== 0 ? "−" : x > 0 && Number(t) !== 0 ? "+" : ""}${t}`;
}

/** Signed number with a real minus. */
export function fmtSigned(v, digits = 1) {
  const t = Math.abs(v).toFixed(digits);
  return `${v < 0 && Number(t) !== 0 ? "−" : v > 0 && Number(t) !== 0 ? "+" : ""}${t}`;
}

export function fmtHz(f) {
  return f >= 1000 ? `${Number((f / 1000).toFixed(f >= 10000 ? 1 : 2))} k` : `${Math.round(f)}`;
}

/** Pan −1…+1 as L50 / C / R50. */
export function fmtPan(p) {
  const v = Math.round(Math.abs(p) * 100);
  return v === 0 ? "C" : `${p < 0 ? "L" : "R"}${v}`;
}

/** Numbers from /metrics (Prometheus text) with the mixer prefix: { name: [{labels, value}] }. */
export function parseMetrics(text) {
  const out = {};
  for (const line of text.split("\n")) {
    const m = /^mxl_audio_mixer_(\w+)(?:\{([^}]*)\})? (\S+)$/.exec(line);
    if (!m) continue;
    const labels = Object.fromEntries([...(m[2] || "").matchAll(/(\w+)="([^"]*)"/g)].map((x) => [x[1], x[2]]));
    (out[m[1]] ||= []).push({ labels, value: Number(m[3]) });
  }
  return out;
}
