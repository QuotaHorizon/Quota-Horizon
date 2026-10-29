import { createElement, Fragment, type ReactNode } from "react";
import type { CodexThreadTimelineItem } from "../../../types";

const escapedQuery = (query: string) => query.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export function highlightedText(value: string, query: string): ReactNode {
  const needle = escapedQuery(query);
  if (!needle) return value;
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const match of value.matchAll(new RegExp(needle, "giu"))) {
    const at = match.index!;
    parts.push(value.slice(cursor, at), <mark key={at}>{match[0]}</mark>);
    cursor = at + match[0].length;
  }
  parts.push(value.slice(cursor));
  return parts;
}

export function matchingItems(items: CodexThreadTimelineItem[], query: string): string[] {
  const needle = escapedQuery(query);
  if (!needle) return [];
  const pattern = new RegExp(needle, "iu");
  return items.filter(item => [item.text, item.toolName, item.toolInput, item.toolOutput]
    .some(value => value && pattern.test(value))).map(item => item.id);
}

export function nextMatch(ids: string[], current: string | null, direction: -1 | 1) {
  if (!ids.length) return null;
  const index = current ? ids.indexOf(current) : -1;
  return ids[index < 0 ? (direction === 1 ? 0 : ids.length - 1) : (index + direction + ids.length) % ids.length];
}

export function safeReadingUrl(value: string): string | null {
  try {
    const parsed = new URL(value);
    return ["http:", "https:"].includes(parsed.protocol) && !parsed.username && !parsed.password ? parsed.href : null;
  } catch { return null; }
}

function inline(value: string, query: string, onOpen?: (url: string) => void): ReactNode {
  // React text nodes keep HTML inert. Links are explicit http(s) only; images stay text.
  const pattern = /(`[^`\n]+`|\*\*[^*\n]+\*\*|__[^_\n]+__|\*[^*\n]+\*|\[[^\]\n]+\]\([^\s)]+\))/g;
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const match of value.matchAll(pattern)) {
    const at = match.index!, token = match[0];
    parts.push(highlightedText(value.slice(cursor, at), query));
    let node: ReactNode;
    if (token.startsWith("`")) node = <code>{highlightedText(token.slice(1, -1), query)}</code>;
    else if (token.startsWith("**") || token.startsWith("__")) node = <strong>{highlightedText(token.slice(2, -2), query)}</strong>;
    else if (token.startsWith("*")) node = <em>{highlightedText(token.slice(1, -1), query)}</em>;
    else {
      const link = /^\[([^\]]+)\]\((.+)\)$/.exec(token)!;
      const url = safeReadingUrl(link[2]);
      node = url ? <a href={url} target="_blank" rel="noopener noreferrer" onClick={event => {
        if (onOpen) { event.preventDefault(); onOpen(url); }
      }}>{highlightedText(link[1], query)}</a> : highlightedText(token, query);
    }
    parts.push(<Fragment key={at}>{node}</Fragment>);
    cursor = at + token.length;
  }
  parts.push(highlightedText(value.slice(cursor), query));
  return parts;
}

const listLine = /^(\s*)([-+*]|\d+[.)])\s+(.+)$/;
const blockStart = (line: string) => /^(?:\s*$|\s*`{3,}|\s*~{3,}|#{1,6}\s|>\s?)/.test(line) || listLine.test(line);

function tableCells(line: string) {
  const cells: string[] = []; let cell = "", inCode = false;
  for (let i = 0; i < line.length; i += 1) {
    if (line[i] === "\\" && line[i + 1] === "|") { cell += "|"; i += 1; }
    else if (line[i] === "`") { inCode = !inCode; cell += line[i]; }
    else if (line[i] === "|" && !inCode) { cells.push(cell.trim()); cell = ""; }
    else cell += line[i];
  }
  cells.push(cell.trim());
  if (line.trim().startsWith("|")) cells.shift();
  if (line.trim().endsWith("|")) cells.pop();
  return cells;
}

/** A small reading renderer using text nodes, never generated HTML. */
export function ReadableMessage({ value, query = "", onOpen }: { value: string; query?: string; onOpen?: (url: string) => void }) {
  const lines = value.replace(/\r\n?/g, "\n").split("\n"), blocks: ReactNode[] = [];
  for (let i = 0; i < lines.length;) {
    const start = i, line = lines[i];
    if (!line.trim()) { i += 1; continue; }
    const fence = /^\s*(`{3,}|~{3,})([^\s]*)\s*$/.exec(line);
    if (fence) {
      const code: string[] = [], delimiter = fence[1][0];
      i += 1;
      while (i < lines.length && !(lines[i].trim().length >= fence[1].length
        && [...lines[i].trim()].every(char => char === delimiter))) code.push(lines[i++]);
      if (i < lines.length) i += 1;
      blocks.push(<pre key={start} data-language={fence[2] || undefined}><code>{highlightedText(code.join("\n"), query)}</code></pre>);
      continue;
    }
    const heading = /^(#{1,6})\s+(.+?)(?:\s+#+)?$/.exec(line);
    if (heading) { blocks.push(createElement(`h${Math.max(3, heading[1].length)}`, { key: start }, inline(heading[2], query, onOpen))); i += 1; continue; }
    const tableHeader = tableCells(line), separator = tableCells(lines[i + 1] ?? "");
    if (tableHeader.length > 1 && separator.length === tableHeader.length && separator.every(cell => /^:?-{3,}:?$/.test(cell))) {
      const rows: string[][] = []; i += 2;
      while (i < lines.length) {
        const cells = tableCells(lines[i]);
        if (cells.length !== tableHeader.length) break;
        rows.push(cells); i += 1;
      }
      blocks.push(<div key={start} style={{ overflowX: "auto" }}><table><thead><tr>{tableHeader.map((cell, c) => <th key={c}>{inline(cell, query, onOpen)}</th>)}</tr></thead>
        <tbody>{rows.map((cells, r) => <tr key={r}>{cells.map((cell, c) => <td key={c}>{inline(cell, query, onOpen)}</td>)}</tr>)}</tbody></table></div>); continue;
    }
    if (/^>\s?/.test(line)) {
      const quote: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i])) quote.push(lines[i++].replace(/^>\s?/, ""));
      blocks.push(<blockquote key={start}>{inline(quote.join("\n"), query, onOpen)}</blockquote>); continue;
    }
    const list = listLine.exec(line);
    if (list) {
      const ordered = /^\d/.test(list[2]), items: ReactNode[] = [];
      let current: RegExpExecArray | null;
      while (i < lines.length && (current = listLine.exec(lines[i])) && /^\d/.test(current[2]) === ordered) {
        items.push(<li key={i}>{inline(current[3], query, onOpen)}</li>); i += 1;
      }
      blocks.push(ordered ? <ol key={start} start={Number.parseInt(list[2], 10)}>{items}</ol> : <ul key={start}>{items}</ul>); continue;
    }
    const paragraph = [line]; i += 1;
    while (i < lines.length && !blockStart(lines[i])) paragraph.push(lines[i++]);
    blocks.push(<p key={start}>{inline(paragraph.join("\n"), query, onOpen)}</p>);
  }
  return <div>{blocks}</div>;
}
