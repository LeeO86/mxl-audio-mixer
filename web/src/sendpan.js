// The per-send surround panner (B6), as the server decides it (Mix::send_pan): a mono or stereo source
// into a 5.1, 7.1, 5.1.4 or 7.1.4 bus can be panned in the send; a post-pan send then reads the strip
// before its stereo panner. A strip with its own surround panner at post-pan is already surround.
import { isBus, signalOf } from "./store.js";

const BEDS = { "5.1": "5.1", "5.1.4": "5.1", "7.1": "7.1", "7.1.4": "7.1" };

/** A new send pan: front centre, no spread, no divergence, LFE off. */
export const DEFAULT_SEND_PAN = { azimuth_deg: 0, spread_pct: 0, divergence_pct: 0, lfe_db: -100 };

/** The surround bed ("5.1" or "7.1") a send's panner pans into, or "" when it has none. */
export function sendPanBed(send) {
  const bed = BEDS[signalOf(send?.to)?.format];
  const src = signalOf(send?.from);
  if (!bed || !src) return "";
  if (!isBus(src.id) && send.tap === "post-pan" && src.chain?.some((n) => n.type === "surround-pan" && !n.bypass)) return "";
  return src.format === "mono" || src.format === "stereo" ? bed : "";
}
