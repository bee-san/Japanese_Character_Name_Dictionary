let lastIndexUrl = '';
let manualEntryCounter = 0;

const VNDB_VN_URL_RE = /vndb\.org\/v\d+/i;
const VNDB_VN_ID_RE = /^v\d+$/i;
const ANILIST_MEDIA_URL_RE = /anilist\.co\/(anime|manga)\/\d+/i;

function activeMode() {
    const activeTab = document.querySelector('.tab.active');
    return activeTab ? activeTab.dataset.tab : 'username';
}

function previewButtons() {
    return ['fetchListsBtn', 'previewManualBtn']
        .map(id => document.getElementById(id))
        .filter(Boolean);
}

function activePreviewButton() {
    return activeMode() === 'manual'
        ? document.getElementById('previewManualBtn')
        : document.getElementById('fetchListsBtn');
}

function setPreviewButtonsDisabled(disabled) {
    previewButtons().forEach(button => {
        button.disabled = disabled;
    });
}

function switchFrequencyTab(mode) {
    document.querySelectorAll('.tab').forEach(tab => {
        const active = tab.dataset.tab === mode;
        tab.classList.toggle('active', active);
        tab.setAttribute('aria-selected', active ? 'true' : 'false');
    });
    document.querySelectorAll('.tab-content').forEach(panel => {
        panel.classList.toggle('active', panel.id === 'tab-' + mode);
    });

    clearUnmatched();
    hideResult();
    hideProgress();
    updateActionButtons();
    updateIndexUrl();
}

function updateActionButtons() {
    const fetchBtn = document.getElementById('fetchListsBtn');
    const previewManualBtn = document.getElementById('previewManualBtn');
    fetchBtn.textContent = 'Fetch Lists & Preview';
    if (previewManualBtn) {
        previewManualBtn.textContent = 'Preview Media List';
    }
}

function buildFrequencyParams({ validate = false } = {}) {
    const params = new URLSearchParams();

    if (activeMode() === 'manual') {
        if (validate && !validateManualEntries()) return null;
        const entries = getManualEntries();
        if (entries.length > 0) {
            params.set('entries', JSON.stringify(entries));
        }
        return params;
    }

    if (validate && !validateUsernameInputs()) return null;
    const vndbUser = document.getElementById('vndbUser').value.trim();
    const anilistUser = document.getElementById('anilistUser').value.trim();

    if (vndbUser) params.set('vndb_user', vndbUser);
    if (anilistUser) params.set('anilist_user', anilistUser);

    return params;
}

async function fetchFrequencyLists() {
    const status = document.getElementById('status');
    const preview = document.getElementById('mediaPreview');
    const fetchBtn = activePreviewButton();

    clearUnmatched();
    hideResult();

    if (activeMode() === 'manual') {
        if (!validateManualEntries()) {
            setStatus(status, 'Fix the media ID warnings before previewing.', 'error');
            return;
        }

        const entries = manualEntriesForPreview();
        if (entries.length === 0) {
            setStatus(status, 'Please enter at least one media ID.', 'error');
            return;
        }

        renderMediaPreview(entries, 'Selected Media');
        updateIndexUrl();
        setStatus(status, `Prepared ${entries.length} title${entries.length === 1 ? '' : 's'} for frequency generation.`, 'success');
        return;
    }

    const params = buildFrequencyParams({ validate: true });
    if (!params || !params.toString()) {
        setStatus(status, 'Please enter at least one VNDB or AniList username.', 'error');
        return;
    }

    fetchBtn.disabled = true;
    fetchBtn.textContent = 'Fetching...';
    preview.classList.remove('show');
    setStatus(status, 'Fetching current VNDB/AniList media...', 'loading');

    try {
        const response = await fetch('/api/user-lists?' + params.toString());
        const data = await response.json();
        if (!response.ok || data.error) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }

        renderMediaPreview(data.entries || [], 'Consumed Media');

        let message = `Found ${(data.entries || []).length} current title${(data.entries || []).length === 1 ? '' : 's'}.`;
        if (data.errors && data.errors.length > 0) {
            message += ` Warnings: ${data.errors.join('; ')}`;
        }
        setStatus(status, message, 'success');
        updateIndexUrl();
    } catch (err) {
        setStatus(status, `Error: ${err.message}`, 'error');
    } finally {
        fetchBtn.disabled = false;
        updateActionButtons();
    }
}

function generateFrequencyDictionary() {
    const params = buildFrequencyParams({ validate: true });
    const status = document.getElementById('status');
    const generateBtn = document.getElementById('generateBtn');
    const progressContainer = document.getElementById('progressContainer');
    const progressBar = document.getElementById('progressBar');

    clearUnmatched();
    hideResult();

    if (!params || !params.toString()) {
        const message = activeMode() === 'manual'
            ? 'Please enter at least one media ID.'
            : 'Please enter at least one VNDB or AniList username.';
        setStatus(status, message, 'error');
        return;
    }

    updateIndexUrl();
    generateBtn.disabled = true;
    setPreviewButtonsDisabled(true);
    generateBtn.textContent = 'Generating...';
    progressContainer.classList.add('show');
    progressBar.style.width = '0%';
    progressBar.setAttribute('aria-valuenow', '0');
    progressBar.textContent = '';
    setStatus(status, 'Starting frequency dictionary generation...', 'loading');

    const eventSource = new EventSource('/api/generate-frequency-stream?' + params.toString());

    eventSource.addEventListener('progress', (event) => {
        const data = JSON.parse(event.data);
        const total = Math.max(Number(data.total) || 1, 1);
        const current = Math.min(Number(data.current) || 0, total);
        const pct = Math.round((current / total) * 100);
        progressBar.style.width = pct + '%';
        progressBar.setAttribute('aria-valuenow', String(pct));
        progressBar.textContent = `${current}/${total}`;
        setStatus(status, `${data.stage}: ${data.title}`, 'loading');
    });

    eventSource.addEventListener('warning', (event) => {
        const data = JSON.parse(event.data);
        if (data.unmatched) {
            renderUnmatched(data.unmatched);
        }
    });

    eventSource.addEventListener('complete', async (event) => {
        eventSource.close();
        const data = JSON.parse(event.data);
        progressBar.style.width = '100%';
        progressBar.setAttribute('aria-valuenow', '100');
        progressBar.textContent = 'Done!';

        if (data.unmatched) {
            renderUnmatched(data.unmatched);
        }

        try {
            const response = await fetch('/api/download?token=' + encodeURIComponent(data.token));
            if (!response.ok) throw new Error('Download failed');

            const blob = await response.blob();
            const downloadUrl = window.URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = downloadUrl;
            a.download = data.filename || 'bee_frequency.zip';
            document.body.appendChild(a);
            a.click();
            a.remove();
            window.URL.revokeObjectURL(downloadUrl);

            showResult();
            const matched = Number(data.matchedCount) || 0;
            const terms = Number(data.termCount) || 0;
            setStatus(status, `Frequency dictionary downloaded. Matched ${matched} title${matched === 1 ? '' : 's'} and combined ${terms} term${terms === 1 ? '' : 's'}.`, 'success');
        } catch (err) {
            setStatus(status, `Download error: ${err.message}`, 'error');
        } finally {
            generateBtn.disabled = false;
            setPreviewButtonsDisabled(false);
            generateBtn.textContent = 'Generate Frequency Dictionary';
            updateActionButtons();
        }
    });

    eventSource.addEventListener('error', (event) => {
        if (event.data) {
            const data = JSON.parse(event.data);
            setStatus(status, `Error: ${data.error}`, 'error');
        } else {
            setStatus(status, 'Connection error. Please try again.', 'error');
        }
        eventSource.close();
        generateBtn.disabled = false;
        setPreviewButtonsDisabled(false);
        generateBtn.textContent = 'Generate Frequency Dictionary';
        hideProgress();
        updateActionButtons();
    });

    eventSource.onerror = () => {
        if (generateBtn.disabled) {
            eventSource.close();
            setStatus(status, 'Connection lost. Please try again.', 'error');
            generateBtn.disabled = false;
            setPreviewButtonsDisabled(false);
            generateBtn.textContent = 'Generate Frequency Dictionary';
            hideProgress();
            updateActionButtons();
        }
    };
}

function addManualEntry() {
    const container = document.getElementById('manualEntries');
    const row = document.createElement('div');
    row.className = 'manual-entry-row';
    row.dataset.index = String(manualEntryCounter++);
    row.innerHTML = `
        <div class="entry-source">
            <label>Source</label>
            <select data-field="source" onchange="onEntrySourceChange(this); updateIndexUrl();">
                <option value="vndb">VNDB</option>
                <option value="anilist">AniList</option>
            </select>
        </div>
        <div class="entry-media-type hidden" data-wrapper="media-type">
            <label>Type</label>
            <select data-field="media_type" onchange="updateIndexUrl()">
                <option value="ANIME">Anime</option>
                <option value="MANGA">Manga / LN</option>
            </select>
        </div>
        <div class="entry-id">
            <label>Media ID</label>
            <input type="text" data-field="id" placeholder="e.g., v17, 9253, or https://anilist.co/anime/9253" oninput="validateManualId(this); updateIndexUrl();">
            <div class="input-hint"></div>
        </div>
        <button type="button" class="remove-entry-btn" onclick="removeManualEntry(this)" title="Remove entry">&times;</button>
    `;
    container.appendChild(row);
    updateRemoveButtons();
    updateIndexUrl();
}

function removeManualEntry(btn) {
    const row = btn.closest('.manual-entry-row');
    row.remove();
    updateRemoveButtons();
    updateIndexUrl();
}

function onEntrySourceChange(select) {
    const row = select.closest('.manual-entry-row');
    const mediaType = row.querySelector('[data-wrapper="media-type"]');
    mediaType.classList.toggle('hidden', select.value !== 'anilist');

    const idInput = row.querySelector('[data-field="id"]');
    if (idInput && idInput.value.trim()) {
        validateManualId(idInput);
    }
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
        if (!id) return;

        const entry = { source, id };
        if (source === 'anilist') {
            entry.media_type = mediaType;
        }
        entries.push(entry);
    });

    return entries;
}

function manualEntriesForPreview() {
    return getManualEntries().map(entry => ({
        source: entry.source,
        id: entry.id,
        title: entry.id,
        title_romaji: entry.id,
        media_type: entry.source === 'vndb'
            ? 'vn'
            : (entry.media_type || 'ANIME').toLowerCase(),
    }));
}

function validateUsernameInputs() {
    return validateVndbUser() && validateAnilistUser();
}

function validateManualEntries() {
    let valid = true;
    document.querySelectorAll('.manual-entry-row').forEach(row => {
        const input = row.querySelector('[data-field="id"]');
        if (input && input.value.trim() && !validateManualId(input)) {
            valid = false;
        }
    });
    return valid;
}

function renderMediaPreview(entries, label = 'Consumed Media') {
    const preview = document.getElementById('mediaPreview');
    const header = document.getElementById('mediaPreviewHeader');
    const list = document.getElementById('mediaPreviewList');

    list.innerHTML = '';
    header.textContent = `${label} (${entries.length})`;

    if (entries.length === 0) {
        preview.classList.remove('show');
        return;
    }

    entries.forEach(entry => {
        const item = document.createElement('div');
        item.className = 'media-item';
        item.innerHTML = mediaMarkup(entry);
        list.appendChild(item);
    });

    preview.classList.add('show');
}

function renderUnmatched(unmatched) {
    const panel = document.getElementById('unmatchedPanel');
    const list = document.getElementById('unmatchedList');

    list.innerHTML = '';
    if (!unmatched || unmatched.length === 0) {
        panel.classList.remove('show');
        return;
    }

    unmatched.forEach(entry => {
        const item = document.createElement('div');
        item.className = 'media-item unmatched-item';
        item.innerHTML = `
            ${mediaMarkup(entry)}
            <div class="unmatched-reason">${escapeHtml(entry.reason || 'No matching Jiten frequency deck found')}</div>
        `;
        list.appendChild(item);
    });

    panel.classList.add('show');
}

function mediaMarkup(entry) {
    const mediaType = String(entry.media_type || '').toLowerCase();
    const badgeClass = entry.source === 'vndb' ? 'vndb' : mediaType;
    const badgeText = entry.source === 'vndb' ? 'VN' :
        mediaType === 'manga' ? 'Manga' : 'Anime';
    const title = entry.title || entry.id || 'Untitled';
    const romaji = entry.title_romaji && entry.title_romaji !== title
        ? `<span class="romaji">${escapeHtml(entry.title_romaji)}</span>`
        : '';

    return `
        <span class="title">${escapeHtml(title)}</span>
        ${romaji}
        <span class="badge ${escapeHtml(badgeClass || '')}">${escapeHtml(badgeText)}</span>
    `;
}

function updateIndexUrl() {
    const params = buildFrequencyParams();
    lastIndexUrl = params && params.toString()
        ? `${location.origin}/api/yomitan-frequency-index?${params.toString()}`
        : '';
    const input = document.getElementById('indexUrl');
    input.value = lastIndexUrl;
    return lastIndexUrl;
}

function showResult() {
    updateIndexUrl();
    if (lastIndexUrl) {
        document.getElementById('resultPanel').classList.add('show');
    }
}

function hideResult() {
    document.getElementById('resultPanel').classList.remove('show');
}

function clearUnmatched() {
    document.getElementById('unmatchedList').innerHTML = '';
    document.getElementById('unmatchedPanel').classList.remove('show');
}

function hideProgress() {
    const progressContainer = document.getElementById('progressContainer');
    const progressBar = document.getElementById('progressBar');
    progressContainer.classList.remove('show');
    progressBar.style.width = '0%';
    progressBar.setAttribute('aria-valuenow', '0');
    progressBar.textContent = '';
}

async function copyIndexUrl() {
    const status = document.getElementById('status');
    const url = updateIndexUrl();
    if (!url) {
        setStatus(status, 'Generate a frequency URL first.', 'error');
        return;
    }

    try {
        await navigator.clipboard.writeText(url);
        setStatus(status, 'Index URL copied.', 'success');
    } catch (err) {
        const input = document.getElementById('indexUrl');
        input.focus();
        input.select();
        setStatus(status, 'Index URL selected.', 'success');
    }
}

function setStatus(el, message, type) {
    el.textContent = message;
    el.className = `status show ${type}`;
}

function setHint(el, input, message, level) {
    el.innerHTML = message;
    el.className = 'input-hint show ' + level;
    input.classList.remove('input-warn', 'input-error');
    if (level === 'warn') input.classList.add('input-warn');
    if (level === 'error') input.classList.add('input-error');
}

function clearHint(el, input) {
    el.innerHTML = '';
    el.className = 'input-hint';
    input.classList.remove('input-warn', 'input-error');
}

function switchToManualTab() {
    switchFrequencyTab('manual');
}

function switchToUsernameTab() {
    switchFrequencyTab('username');
}

function validateVndbUser() {
    const input = document.getElementById('vndbUser');
    const hint = document.getElementById('vndbUserHint');
    const val = input.value.trim();

    if (!val) {
        clearHint(hint, input);
        return true;
    }

    if (VNDB_VN_URL_RE.test(val) || VNDB_VN_ID_RE.test(val)) {
        const label = VNDB_VN_URL_RE.test(val) ? 'a VN URL' : 'a VN ID';
        setHint(hint, input, `This looks like ${label}, not a username. Use the <a onclick="switchToManualTab()">Media ID tab</a> instead.`, 'warn');
        return false;
    }

    clearHint(hint, input);
    return true;
}

function validateAnilistUser() {
    const input = document.getElementById('anilistUser');
    const hint = document.getElementById('anilistUserHint');
    const val = input.value.trim();

    if (!val) {
        clearHint(hint, input);
        return true;
    }

    if (ANILIST_MEDIA_URL_RE.test(val)) {
        setHint(hint, input, 'This looks like a media URL, not a username. Use the <a onclick="switchToManualTab()">Media ID tab</a> instead.', 'warn');
        return false;
    }

    if (/^\d+$/.test(val)) {
        setHint(hint, input, 'This looks like a media ID, not a username. Use the <a onclick="switchToManualTab()">Media ID tab</a> if you meant a media ID.', 'warn');
        return false;
    }

    clearHint(hint, input);
    return true;
}

function validateManualId(input) {
    const row = input.closest('.manual-entry-row');
    const sourceSelect = row.querySelector('[data-field="source"]');
    const hint = row.querySelector('.entry-id .input-hint');
    const val = input.value.trim();

    if (!val) {
        clearHint(hint, input);
        return true;
    }

    if (/vndb\.org\/u\d+/i.test(val) || /^u\d+$/i.test(val)) {
        setHint(hint, input, 'This looks like a VNDB user ID. Use the <a onclick="switchToUsernameTab()">Username tab</a> for user-based generation.', 'warn');
        return false;
    }

    if (/anilist\.co\/user\//i.test(val)) {
        setHint(hint, input, 'This looks like a user profile URL. Use the <a onclick="switchToUsernameTab()">Username tab</a> for user-based generation.', 'warn');
        return false;
    }

    if (VNDB_VN_URL_RE.test(val)) {
        if (sourceSelect.value !== 'vndb') {
            sourceSelect.value = 'vndb';
            onEntrySourceChange(sourceSelect);
        }
        clearHint(hint, input);
        return true;
    }

    if (ANILIST_MEDIA_URL_RE.test(val)) {
        if (sourceSelect.value !== 'anilist') {
            sourceSelect.value = 'anilist';
            onEntrySourceChange(sourceSelect);
        }
        const match = val.match(/anilist\.co\/(anime|manga)\/\d+/i);
        const mediaTypeSelect = row.querySelector('[data-field="media_type"]');
        if (match && mediaTypeSelect) {
            mediaTypeSelect.value = match[1].toUpperCase() === 'ANIME' ? 'ANIME' : 'MANGA';
        }
        clearHint(hint, input);
        return true;
    }

    if (sourceSelect.value === 'vndb') {
        if (VNDB_VN_ID_RE.test(val) || /^\d+$/.test(val)) {
            clearHint(hint, input);
            return true;
        }
        setHint(hint, input, 'Expected a VNDB VN ID like <b>v17</b>, <b>17</b>, or a vndb.org URL.', 'error');
        return false;
    }

    if (/^\d+$/.test(val)) {
        clearHint(hint, input);
        return true;
    }

    setHint(hint, input, 'Expected a numeric AniList ID like <b>9253</b> or an anilist.co URL.', 'error');
    return false;
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text == null ? '' : String(text);
    return div.innerHTML;
}

document.addEventListener('DOMContentLoaded', () => {
    document.querySelectorAll('.tab').forEach(tab => {
        tab.addEventListener('click', () => switchFrequencyTab(tab.dataset.tab));
    });
    addManualEntry();
    updateActionButtons();
    updateIndexUrl();
});
