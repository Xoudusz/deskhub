// HTML builders for the settings cards (copied from Atreia settingsWindow.ts / views.ts slider()).
import { icon } from "./icons";

export const esc = (s: string) => s.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

export const card = (title: string, sub: string, rows: string) =>
  `<section class="swc"><div class="swch"><b>${esc(title)}</b>${sub ? `<span class="sub">${esc(sub)}</span>` : ""}</div>${rows}</section>`;

/** `label` and `hint` are trusted HTML (callers escape their own text); `ctrl` is HTML. */
export const row = (label: string, ctrl: string, hint = "") =>
  `<div class="swr"><div class="swl">${label}${hint ? `<div class="sub">${hint}</div>` : ""}</div>${ctrl ? `<div class="swx">${ctrl}</div>` : ""}</div>`;

export const tgl = (attr: string, on: boolean, disabled = false) =>
  `<label class="tgl"><input type="checkbox" ${attr} ${on ? "checked" : ""} ${disabled ? "disabled" : ""}><i></i></label>`;

export const text = (t: string) => `<div class="swr"><div class="swl sub">${esc(t)}</div></div>`;

/** One slider per line with − / + buttons. `key` ties the input, the value label and the step buttons together. */
export const slider = (key: string, label: string, min: number, max: number, value: number, hint = "") =>
  `<div class="swr swsl"><div class="sl">` +
  `<div class="slh"><span>${esc(label)}</span><b data-val="${key}">${value}%</b></div>` +
  `<div class="slc">` +
  `<button type="button" class="ib slb" data-act="step" data-slider="${key}" data-d="-1" title="Less">${icon("minus", 11)}</button>` +
  `<input type="range" min="${min}" max="${max}" step="1" value="${value}" data-slider="${key}" aria-label="${esc(label)}">` +
  `<button type="button" class="ib slb" data-act="step" data-slider="${key}" data-d="1" title="More">${icon("plus", 11)}</button>` +
  `</div>${hint ? `<div class="sub sln">${esc(hint)}</div>` : ""}</div></div>`;

/** A select with [value, label] options. */
export const select = (attr: string, options: [string, string][], current: string, disabled = false) =>
  `<select ${attr} ${disabled ? "disabled" : ""}>${options.map(([v, l]) => `<option value="${esc(v)}" ${v === current ? "selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
