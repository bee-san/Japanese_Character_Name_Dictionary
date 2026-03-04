// === Dictionary Settings State ===
const settings = {
    honorifics: true,
    image: true,
    tag: true,
    description: true,
    traits: true,
    spoilers: true,
    seiyuu: true,
};

function toggleSetting(key) {
    settings[key] = !settings[key];

    // If description is disabled, spoilers implicitly off
    if (key === 'description' && !settings.description) {
        settings.spoilers = true; // reset so re-enabling description shows spoilers
    }

    updatePreviewCard();
}

function updatePreviewCard() {
    const sections = {
        honorifics: document.getElementById('preview-honorifics'),
        image: document.getElementById('preview-image'),
        tag: document.getElementById('preview-tag'),
        description: document.getElementById('preview-description'),
        traits: document.getElementById('preview-traits'),
        seiyuu: document.getElementById('preview-seiyuu'),
    };

    for (const [key, el] of Object.entries(sections)) {
        if (!el) continue;
        const enabled = settings[key];
        el.classList.toggle('disabled', !enabled);
        const btn = el.querySelector('.toggle-btn');
        if (btn) {
            btn.textContent = enabled ? '\u274c' : '\u2705';
            btn.title = enabled ? 'Click to disable' : 'Click to re-enable';
        }
    }

    // Grey out the ちゃん suffix in the header when honorifics are off
    const honorificSuffix = document.getElementById('header-honorific-suffix');
    if (honorificSuffix) {
        honorificSuffix.style.color = settings.honorifics ? '' : '#ccc';
    }

    // Dim the "primary" tag in the header when role badge (tag) is off
    const primaryTag = document.getElementById('header-primary-tag');
    if (primaryTag) {
        primaryTag.style.opacity = settings.tag ? '' : '0.2';
        primaryTag.style.filter = settings.tag ? '' : 'grayscale(1)';
    }

    // Spoiler toggle (nested under description)
    const spoilerEl = document.getElementById('preview-spoilers');
    const spoilerWrapper = document.getElementById('spoiler-wrapper');
    if (spoilerWrapper) {
        // Hide the whole spoiler block when description is off
        spoilerWrapper.style.display = settings.description ? '' : 'none';
    }
    if (spoilerEl) {
        const enabled = settings.spoilers;
        spoilerEl.classList.toggle('disabled', !enabled);
        const btn = spoilerEl.querySelector('.toggle-btn');
        if (btn) {
            btn.textContent = enabled ? '\u274c' : '\u2705';
            btn.title = enabled ? 'Click to disable (hide spoilers)' : 'Click to re-enable (show spoilers)';
        }
    }
}

// Build query param string from non-default settings
function settingsParams() {
    const parts = [];
    if (!settings.honorifics) parts.push('honorifics=false');
    if (!settings.image) parts.push('image=false');
    if (!settings.tag) parts.push('tag=false');
    if (!settings.description) parts.push('description=false');
    if (!settings.traits) parts.push('traits=false');
    if (!settings.spoilers) parts.push('spoilers=false');
    if (!settings.seiyuu) parts.push('seiyuu=false');
    return parts.join('&');
}

// === Fetch and display build timestamp ===
fetch('/api/build-info')
    .then(r => r.json())
    .then(data => {
        if (data.build_time && data.build_time !== 'unknown') {
            document.getElementById('buildInfo').textContent = 'Build: ' + data.build_time;
        }
    })
    .catch(() => {});

// === Tab switching ===
document.querySelectorAll('.tab').forEach(tab => {
    tab.addEventListener('click', () => {
        document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
        document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
        tab.classList.add('active');
        document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
    });
});

// === Manual tab: dynamic entry rows ===
let manualEntryCounter = 0;

function addManualEntry() {
    const container = document.getElementById('manualEntries');
    const row = document.createElement('div');
    row.className = 'manual-entry-row';
    const idx = manualEntryCounter++;

    row.innerHTML = `
        <div class="entry-source">
            <label>Source</label>
            <select data-field="source" onchange="onEntrySourceChange(this)">
                <option value="vndb">VNDB</option>
                <option value="anilist">AniList</option>
            </select>
        </div>
        <div class="entry-media-type hidden" data-wrapper="media-type">
            <label>Type</label>
            <select data-field="media_type">
                <option value="ANIME">Anime</option>
                <option value="MANGA">Manga / LN</option>
            </select>
        </div>
        <div class="entry-id">
            <label>Media ID</label>
            <input type="text" data-field="id" placeholder="e.g., v17, 9253, or https://anilist.co/anime/9253">
        </div>
        <button type="button" class="remove-entry-btn" onclick="removeManualEntry(this)" title="Remove entry">&times;</button>
    `;

    container.appendChild(row);
    updateRemoveButtons();
}

function removeManualEntry(btn) {
    const row = btn.closest('.manual-entry-row');
    row.remove();
    updateRemoveButtons();
}

function onEntrySourceChange(select) {
    const row = select.closest('.manual-entry-row');
    const mtWrapper = row.querySelector('[data-wrapper="media-type"]');
    mtWrapper.classList.toggle('hidden', select.value !== 'anilist');
}

function updateRemoveButtons() {
    const rows = document.querySelectorAll('.manual-entry-row');
    rows.forEach(row => {
        const btn = row.querySelector('.remove-entry-btn');
        btn.classList.toggle('hidden', rows.length <= 1);
    });
}

function getManualEntries() {
    const rows = document.querySelectorAll('.manual-entry-row');
    const entries = [];
    rows.forEach(row => {
        const source = row.querySelector('[data-field="source"]').value;
        const id = row.querySelector('[data-field="id"]').value.trim();
        const mediaType = row.querySelector('[data-field="media_type"]').value;
        if (id) {
            const entry = { source, id };
            if (source === 'anilist') {
                entry.media_type = mediaType;
            }
            entries.push(entry);
        }
    });
    return entries;
}

// === Shared generate button: dispatch based on active tab ===
function generateDictionary() {
    const activeTab = document.querySelector('.tab.active');
    if (activeTab && activeTab.dataset.tab === 'manual') {
        generateFromManual();
    } else {
        generateFromUsername();
    }
}

// === Username tab: Fetch lists ===
async function fetchLists() {
    const vndbUser = document.getElementById('vndbUser').value.trim();
    const anilistUser = document.getElementById('anilistUser').value.trim();
    const status = document.getElementById('statusUsername');
    const fetchBtn = document.getElementById('fetchListsBtn');
    const preview = document.getElementById('mediaPreview');

    if (!vndbUser && !anilistUser) {
        status.textContent = 'Please enter at least one username.';
        status.className = 'status show error';
        return;
    }

    fetchBtn.disabled = true;
    fetchBtn.textContent = 'Fetching...';
    status.textContent = 'Fetching user lists...';
    status.className = 'status show loading';
    preview.classList.remove('show');

    try {
        let url = '/api/user-lists?';
        const params = [];
        if (vndbUser) params.push('vndb_user=' + encodeURIComponent(vndbUser));
        if (anilistUser) params.push('anilist_user=' + encodeURIComponent(anilistUser));
        url += params.join('&');

        const response = await fetch(url);
        const data = await response.json();

        if (data.error) {
            throw new Error(data.error);
        }

        const entries = data.entries || [];

        if (entries.length === 0) {
            status.textContent = 'No in-progress media found. Make sure you have titles marked as "Playing" (VNDB) or "Watching/Reading" (AniList).';
            status.className = 'status show error';
            return;
        }

        // Show preview
        const header = document.getElementById('mediaPreviewHeader');
        header.textContent = `In-Progress Media (${entries.length})`;

        const list = document.getElementById('mediaPreviewList');
        list.innerHTML = '';

        entries.forEach(entry => {
            const item = document.createElement('div');
            item.className = 'media-item';

            const badgeClass = entry.source === 'vndb' ? 'vndb' : entry.media_type;
            const badgeText = entry.source === 'vndb' ? 'VN' :
                entry.media_type === 'anime' ? 'Anime' : 'Manga';

            item.innerHTML = `
                <span class="title">${escapeHtml(entry.title)}</span>
                ${entry.title_romaji && entry.title_romaji !== entry.title
                    ? `<span class="romaji">${escapeHtml(entry.title_romaji)}</span>`
                    : ''}
                <span class="badge ${badgeClass}">${badgeText}</span>
            `;
            list.appendChild(item);
        });

        preview.classList.add('show');

        let msg = `Found ${entries.length} in-progress title${entries.length !== 1 ? 's' : ''}.`;
        if (data.errors && data.errors.length > 0) {
            msg += ` (Warnings: ${data.errors.join('; ')})`;
        }
        status.textContent = msg;
        status.className = 'status show success';

    } catch (err) {
        status.textContent = `Error: ${err.message}`;
        status.className = 'status show error';
    } finally {
        fetchBtn.disabled = false;
        fetchBtn.textContent = 'Fetch Lists & Preview';
    }
}

// === Username tab: Generate dictionary with SSE progress ===
function generateFromUsername() {
    const vndbUser = document.getElementById('vndbUser').value.trim();
    const anilistUser = document.getElementById('anilistUser').value.trim();
    const status = document.getElementById('statusUsername');
    const genBtn = document.getElementById('generateBtn');
    const fetchBtn = document.getElementById('fetchListsBtn');
    const progressContainer = document.getElementById('progressContainer');
    const progressBar = document.getElementById('progressBar');

    genBtn.disabled = true;
    genBtn.textContent = 'Generating...';
    fetchBtn.disabled = true;
    progressContainer.classList.add('show');
    progressBar.style.width = '0%';
    progressBar.textContent = '';
    status.innerHTML = 'Starting dictionary generation... Sorry for the wait — we rate-limit our requests to be kind to VNDB and AniList, who generously provide their APIs for free.';
    status.className = 'status show loading';

    let url = '/api/generate-stream?';
    const params = [];
    if (vndbUser) params.push('vndb_user=' + encodeURIComponent(vndbUser));
    if (anilistUser) params.push('anilist_user=' + encodeURIComponent(anilistUser));
    const sp = settingsParams();
    if (sp) params.push(sp);
    url += params.join('&');

    const eventSource = new EventSource(url);

    eventSource.addEventListener('progress', (e) => {
        const data = JSON.parse(e.data);
        const pct = Math.round((data.current / data.total) * 100);
        progressBar.style.width = pct + '%';
        progressBar.textContent = `${data.current}/${data.total}`;
        status.textContent = `Processing ${data.current}/${data.total}: ${data.title}`;
        status.className = 'status show loading';
    });

    eventSource.addEventListener('complete', async (e) => {
        eventSource.close();
        const data = JSON.parse(e.data);
        progressBar.style.width = '100%';
        progressBar.textContent = 'Done!';
        status.textContent = 'Downloading dictionary...';

        try {
            const response = await fetch('/api/download?token=' + encodeURIComponent(data.token));
            if (!response.ok) throw new Error('Download failed');

            const blob = await response.blob();
            const downloadUrl = window.URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = downloadUrl;
            a.download = 'bee_characters.zip';
            document.body.appendChild(a);
            a.click();
            a.remove();
            window.URL.revokeObjectURL(downloadUrl);

            status.textContent = 'Dictionary downloaded! Import the ZIP into Yomitan.';
            status.className = 'status show success';
        } catch (err) {
            status.textContent = `Download error: ${err.message}`;
            status.className = 'status show error';
        } finally {
            genBtn.disabled = false;
            genBtn.textContent = 'Generate Dictionary';
            fetchBtn.disabled = false;
        }
    });

    eventSource.addEventListener('error', (e) => {
        if (e.data) {
            const data = JSON.parse(e.data);
            status.textContent = `Error: ${data.error}`;
        } else {
            status.textContent = 'Connection error. Please try again.';
        }
        status.className = 'status show error';
        eventSource.close();
        genBtn.disabled = false;
        genBtn.textContent = 'Generate Dictionary';
        fetchBtn.disabled = false;
        progressContainer.classList.remove('show');
    });

    eventSource.onerror = () => {
        if (genBtn.disabled) {
            eventSource.close();
            status.textContent = 'Connection lost. Please try again.';
            status.className = 'status show error';
            genBtn.disabled = false;
            genBtn.textContent = 'Generate Dictionary';
            fetchBtn.disabled = false;
            progressContainer.classList.remove('show');
        }
    };
}

// === Manual tab: Media generation (single or multi-entry) ===
async function generateFromManual() {
    const entries = getManualEntries();
    const genBtn = document.getElementById('generateBtn');
    const status = document.getElementById('statusManual');

    if (entries.length === 0) {
        status.textContent = 'Please enter at least one media ID.';
        status.className = 'status show error';
        return;
    }

    genBtn.disabled = true;
    genBtn.textContent = 'Generating...';
    status.innerHTML = 'Fetching characters and building dictionary... Sorry for the wait — we rate-limit our requests to be kind to VNDB and AniList, who generously provide their APIs for free.';
    status.className = 'status show loading';

    try {
        let url;
        if (entries.length === 1) {
            // Single entry: use backward-compatible source+id params
            const e = entries[0];
            url = `/api/yomitan-dict?source=${e.source}&id=${encodeURIComponent(e.id)}`;
            if (e.source === 'anilist' && e.media_type) {
                url += `&media_type=${e.media_type}`;
            }
        } else {
            // Multiple entries: encode as JSON array
            url = `/api/yomitan-dict?entries=${encodeURIComponent(JSON.stringify(entries))}`;
        }
        const sp = settingsParams();
        if (sp) url += '&' + sp;

        const response = await fetch(url);

        if (!response.ok) {
            const text = await response.text();
            throw new Error(text || `HTTP ${response.status}`);
        }

        const blob = await response.blob();
        const downloadUrl = window.URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = downloadUrl;
        if (entries.length === 1) {
            a.download = `yomitan_${entries[0].source}_${entries[0].id}.zip`;
        } else {
            a.download = 'bee_characters.zip';
        }
        document.body.appendChild(a);
        a.click();
        a.remove();
        window.URL.revokeObjectURL(downloadUrl);

        status.textContent = 'Dictionary downloaded! Import the ZIP into Yomitan.';
        status.className = 'status show success';
    } catch (err) {
        status.textContent = `Error: ${err.message}`;
        status.className = 'status show error';
    } finally {
        genBtn.disabled = false;
        genBtn.textContent = 'Generate Dictionary';
    }
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// Initialize the preview card toggles and first manual entry row on load
document.addEventListener('DOMContentLoaded', () => {
    updatePreviewCard();
    addManualEntry();
});
