"use strict";

// ── Constants ──────────────────────────────────────────────────────────────

const AUTHOR_TAG   = "Bee's Custom Yomitan Dict Maker";
const TAG_NAME     = "custom";
const TERM_LIMIT   = 2000;
const DB_NAME      = "BeesYomitanDicts";
const DB_VERSION   = 1;
const STORE_NAME   = "dictionaries";

// ── IndexedDB ──────────────────────────────────────────────────────────────

let db = null;

function openDB() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = e => {
      const d = e.target.result;
      if (!d.objectStoreNames.contains(STORE_NAME)) {
        const store = d.createObjectStore(STORE_NAME, { keyPath: "id" });
        store.createIndex("created_at", "created_at", { unique: false });
      }
    };
    req.onsuccess = e => resolve(e.target.result);
    req.onerror   = e => reject(e.target.error);
  });
}

function dbTx(mode, fn) {
  return new Promise((resolve, reject) => {
    const tx    = db.transaction(STORE_NAME, mode);
    const store = tx.objectStore(STORE_NAME);
    let result;
    fn(store).onsuccess = e => { result = e.target.result; };
    tx.oncomplete = () => resolve(result);
    tx.onerror    = () => reject(tx.error);
    tx.onabort    = () => reject(tx.error);
  });
}

function saveDict(record) {
  return dbTx("readwrite", s => s.put(record));
}

function getAllDicts() {
  return new Promise((resolve, reject) => {
    const tx    = db.transaction(STORE_NAME, "readonly");
    const store = tx.objectStore(STORE_NAME);
    const req   = store.index("created_at").getAll();
    req.onsuccess = e => resolve(e.target.result.reverse()); // newest first
    req.onerror   = e => reject(e.target.error);
  });
}

function getDict(id) {
  return dbTx("readonly", s => s.get(id));
}

function deleteDict(id) {
  return dbTx("readwrite", s => s.delete(id));
}

function incrementHitCount(id) {
  return new Promise((resolve, reject) => {
    const tx    = db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);
    const req   = store.get(id);
    req.onsuccess = e => {
      const record = e.target.result;
      if (!record) return resolve();
      record.hit_count = (record.hit_count || 0) + 1;
      const put = store.put(record);
      put.onsuccess = () => resolve();
      put.onerror   = err => reject(err.target ? err.target.error : err);
    };
    req.onerror = e => reject(e.target.error);
  });
}

// ── Entry parsing ──────────────────────────────────────────────────────────

function parseEntries(rawText) {
  const entries = [];
  for (const raw of rawText.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const firstComma  = line.indexOf(",");
    if (firstComma === -1) continue;
    const secondComma = line.indexOf(",", firstComma + 1);
    if (secondComma === -1) continue;

    const term       = line.slice(0, firstComma).trim();
    const reading    = line.slice(firstComma + 1, secondComma).trim();
    const definition = line.slice(secondComma + 1).trim();
    if (!term) continue;
    entries.push({ term, reading, definition });
  }
  return entries;
}

// ── Yomitan ZIP builder ────────────────────────────────────────────────────

async function buildZip(dictName, entries) {
  const zip      = new JSZip();
  const revision = Math.floor(Date.now() / 1000);

  zip.file("index.json", JSON.stringify({
    title:    dictName,
    author:   AUTHOR_TAG,
    format:   3,
    revision: String(revision),
  }, null, 2));

  zip.file("tag_bank_1.json", JSON.stringify([
    [TAG_NAME, "meta", 0, AUTHOR_TAG, 0],
  ]));

  for (let i = 0; i < entries.length; i += TERM_LIMIT) {
    const chunk     = entries.slice(i, i + TERM_LIMIT);
    const bankIndex = Math.floor(i / TERM_LIMIT) + 1;
    const bank      = chunk.map(({ term, reading, definition }) =>
      [term, reading, TAG_NAME, "", 0, [definition], 0, ""]
    );
    zip.file(`term_bank_${bankIndex}.json`, JSON.stringify(bank));
  }

  return zip.generateAsync({ type: "blob", compression: "DEFLATE", compressionOptions: { level: 6 } });
}

// ── Yomitan ZIP parser ─────────────────────────────────────────────────────

async function parseZip(file) {
  let zip;
  try {
    zip = await JSZip.loadAsync(file);
  } catch {
    throw new Error("Could not read ZIP file. Is it a valid .zip?");
  }

  const indexFile = zip.file("index.json");
  if (!indexFile) throw new Error("Missing index.json — not a valid Yomitan dictionary.");

  let indexData;
  try {
    indexData = JSON.parse(await indexFile.async("string"));
  } catch {
    throw new Error("index.json is malformed.");
  }

  if (indexData.author !== AUTHOR_TAG) {
    throw new Error("This dictionary was not created by Bee's Custom Yomitan Dict Maker and cannot be edited here.");
  }

  const bankFiles = Object.keys(zip.files)
    .filter(n => /^term_bank_\d+\.json$/.test(n))
    .sort((a, b) => {
      const na = parseInt(a.match(/\d+/)[0], 10);
      const nb = parseInt(b.match(/\d+/)[0], 10);
      return na - nb;
    });

  if (bankFiles.length === 0) throw new Error("No term data found in dictionary.");

  const lines = [];
  for (const fname of bankFiles) {
    let bank;
    try {
      bank = JSON.parse(await zip.file(fname).async("string"));
    } catch {
      throw new Error(`Could not parse ${fname}.`);
    }
    for (const entry of bank) {
      // entry: [term, reading, tags, rules, score, [defs], seq, termTags]
      const term       = entry[0] ?? "";
      const reading    = entry[1] ?? "";
      const defs       = entry[5];
      const definition = Array.isArray(defs) ? defs.join(" / ") : (defs ?? "");
      lines.push(`${term}, ${reading}, ${definition}`);
    }
  }

  return {
    dictName: indexData.title || "Imported Dictionary",
    rawText:  lines.join("\n"),
  };
}

// ── UI helpers ─────────────────────────────────────────────────────────────

function setStatus(msg, type = "info", autoClear = true) {
  const bar = document.getElementById("status-bar");
  bar.className = `show status-${type}`;
  bar.textContent = msg;
  if (autoClear) {
    clearTimeout(bar._timer);
    bar._timer = setTimeout(() => {
      bar.textContent = "";
      bar.className   = "";
    }, 4000);
  }
}

function setLoading(loading) {
  document.getElementById("btn-generate").disabled = loading;
  document.getElementById("btn-upload-trigger").disabled = loading;
  document.getElementById("btn-clear").disabled = loading;
}

function updateEntryCount() {
  const raw     = document.getElementById("entry-area").value;
  const count   = parseEntries(raw).length;
  const display = document.getElementById("entry-count");
  display.textContent = count === 1 ? "1 entry" : `${count} entries`;
}

function triggerDownload(blob, filename) {
  const url = URL.createObjectURL(blob);
  const a   = document.createElement("a");
  a.href     = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 5000);
}

// ── History sidebar ────────────────────────────────────────────────────────

async function renderHistory() {
  const list  = document.getElementById("history-list");
  const dicts = await getAllDicts();

  if (dicts.length === 0) {
    list.innerHTML = `<div class="history-empty">No dictionaries yet.<br>Generate one to see it here.</div>`;
    return;
  }

  list.innerHTML = "";
  for (const d of dicts) {
    const item = document.createElement("div");
    item.className   = "history-item";
    item.dataset.id  = d.id;

    const date = new Date(d.created_at).toLocaleDateString(undefined, {
      month: "short", day: "numeric", year: "numeric"
    });
    const entryCount = parseEntries(d.raw_text).length;

    item.innerHTML = `
      <div class="history-item-name" title="${escHtml(d.dict_name)}">${escHtml(d.dict_name)}</div>
      <div class="history-item-meta">
        <span>${entryCount} ${entryCount === 1 ? "entry" : "entries"}</span>
        <span>${date}</span>
      </div>
      <div class="history-item-actions">
        <button class="btn-ghost btn-hist-load" data-id="${d.id}">Load &amp; Edit</button>
        <button class="btn-ghost btn-hist-dl"   data-id="${d.id}">Download</button>
        <button class="btn-ghost btn-hist-del"  data-id="${d.id}" style="color:var(--error)">Delete</button>
      </div>
    `;
    list.appendChild(item);
  }

  list.querySelectorAll(".history-item").forEach(item =>
    item.addEventListener("click", () => histLoad(item.dataset.id))
  );
  list.querySelectorAll(".btn-hist-load").forEach(btn =>
    btn.addEventListener("click", e => { e.stopPropagation(); histLoad(btn.dataset.id); })
  );
  list.querySelectorAll(".btn-hist-dl").forEach(btn =>
    btn.addEventListener("click", e => { e.stopPropagation(); histDownload(btn.dataset.id); })
  );
  list.querySelectorAll(".btn-hist-del").forEach(btn =>
    btn.addEventListener("click", e => { e.stopPropagation(); histDelete(btn.dataset.id); })
  );
}

function escHtml(str) {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

async function histLoad(id) {
  const record = await getDict(id);
  if (!record) return;
  document.getElementById("dict-name").value  = record.dict_name;
  document.getElementById("entry-area").value = record.raw_text;
  updateEntryCount();
  setStatus(`Loaded "${record.dict_name}" for editing.`, "success");
  document.getElementById("entry-area").focus();
}

async function histDownload(id) {
  const record = await getDict(id);
  if (!record) return;
  await incrementHitCount(id);
  const blob = new Blob([record.zip_blob], { type: "application/zip" });
  const safe = record.dict_name.replace(/[^a-zA-Z0-9\u3000-\u9fff\s_-]/g, "").trim() || "dictionary";
  triggerDownload(blob, `${safe}.zip`);
  await renderHistory();
}

async function histDelete(id) {
  await deleteDict(id);
  await renderHistory();
  setStatus("Dictionary deleted.", "info");
}

// ── Generate flow ──────────────────────────────────────────────────────────

async function handleGenerate() {
  const dictName = document.getElementById("dict-name").value.trim();
  const rawText  = document.getElementById("entry-area").value;

  if (!dictName) {
    setStatus("Please enter a dictionary name.", "error");
    document.getElementById("dict-name").focus();
    return;
  }

  const entries = parseEntries(rawText);
  if (entries.length === 0) {
    setStatus("No valid entries found. Format: term, reading, definition", "error");
    return;
  }

  setLoading(true);
  setStatus("Building dictionary…", "info", false);

  try {
    const blob   = await buildZip(dictName, entries);
    const arrBuf = await blob.arrayBuffer();

    const record = {
      id:         crypto.randomUUID(),
      dict_name:  dictName,
      raw_text:   rawText,
      zip_blob:   arrBuf,      // stored as ArrayBuffer
      created_at: new Date().toISOString(),
      hit_count:  0,
    };
    await saveDict(record);
    await renderHistory();

    const safe = dictName.replace(/[^a-zA-Z0-9\u3000-\u9fff\s_-]/g, "").trim() || "dictionary";
    triggerDownload(blob, `${safe}.zip`);

    setStatus(`✓ "${dictName}" downloaded (${entries.length} ${entries.length === 1 ? "entry" : "entries"}).`, "success");
  } catch (err) {
    setStatus(`Error: ${err.message}`, "error");
    console.error(err);
  } finally {
    setLoading(false);
  }
}

// ── Upload flow ────────────────────────────────────────────────────────────

async function handleUpload(file) {
  if (!file) return;
  setLoading(true);
  setStatus("Reading dictionary…", "info", false);

  try {
    const { dictName, rawText } = await parseZip(file);
    document.getElementById("dict-name").value  = dictName;
    document.getElementById("entry-area").value = rawText;
    updateEntryCount();
    setStatus(`✓ Loaded "${dictName}". Edit and re-generate to save changes.`, "success");
    document.getElementById("entry-area").focus();
  } catch (err) {
    setStatus(err.message, "error");
    console.error(err);
  } finally {
    setLoading(false);
    // Reset so the same file can be re-uploaded if needed
    document.getElementById("file-input").value = "";
  }
}

// ── ChatGPT prompt collapse ─────────────────────────────────────────────────

function initPromptCollapse() {
  const tip = document.querySelector(".chatgpt-tip");
  if (!tip) return;
  try {
    if (localStorage.getItem("beeCustomDict_promptCollapsed") === "true") {
      tip.classList.add("collapsed");
    }
  } catch (e) {}
}

function togglePromptCollapse() {
  const tip = document.querySelector(".chatgpt-tip");
  if (!tip) return;
  const collapsed = tip.classList.toggle("collapsed");
  try {
    localStorage.setItem("beeCustomDict_promptCollapsed", collapsed ? "true" : "false");
  } catch (e) {}
}

// ── Init ───────────────────────────────────────────────────────────────────

async function init() {
  db = await openDB();
  await renderHistory();
  initPromptCollapse();

  const textarea = document.getElementById("entry-area");
  textarea.addEventListener("input", updateEntryCount);
  updateEntryCount();

  document.getElementById("btn-generate").addEventListener("click", handleGenerate);

  document.getElementById("btn-upload-trigger").addEventListener("click", () =>
    document.getElementById("file-input").click()
  );

  document.getElementById("file-input").addEventListener("change", e => {
    const file = e.target.files[0];
    if (file) handleUpload(file);
  });

  document.getElementById("btn-clear").addEventListener("click", () => {
    if (!document.getElementById("entry-area").value.trim()) return;
    if (!confirm("Clear all entries?")) return;
    document.getElementById("entry-area").value = "";
    updateEntryCount();
  });
}

document.addEventListener("DOMContentLoaded", init);
