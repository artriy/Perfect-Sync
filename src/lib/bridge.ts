import type { DiffItem } from "./types";
import { SAMPLE_DIFF } from "../data/mock";

export interface Preview {
  name: string;
  items: DiffItem[];
}

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Decode a PERFECT- code into an apply-preview. Real codec under Tauri; mock in the browser. */
export async function previewCode(code: string, installed: [string, string][]): Promise<Preview> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Preview>("preview_code", { code, installed });
  }
  // browser fallback so `pnpm dev` still demos the flow
  await new Promise((r) => setTimeout(r, 500));
  return { name: "Lobby - TownOfUs Night", items: SAMPLE_DIFF };
}
