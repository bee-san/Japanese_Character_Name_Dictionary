let lastIndexUrl = '';
let manualEntryCounter = 0;

const VNDB_VN_URL_RE = /vndb\.org\/v\d+/i;
const VNDB_VN_ID_RE = /^v\d+$/i;
const ANILIST_MEDIA_URL_RE = /anilist\.co\/(anime|manga)\/\d+/i;
const DEFAULT_VNDB_STATUSES = ['playing'];
const DEFAULT_ANILIST_STATUSES = ['current'];
const STATUS_LABELS = {
    playing: 'Current',
    current: 'Current',
    finished: 'Completed',
    completed: 'Completed',
    wishlist: 'Wishlist',
    planning: 'Planning',
    paused: 'Paused',
    dropped: 'Dropped',
};
const SAMPLE_FREQUENCY_DECKS = [
    { title: 'Cozy VN', count: 30, total: 10000 },
    { title: 'School anime', count: 5, total: 5000 },
    { title: 'Mystery manga', count: 0, total: 8000 },
];

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
        tab.tabIndex = active ? 0 : -1;
    });
    document.querySelectorAll('.tab-content').forEach(panel => {
        const active = panel.id === 'tab-' + mode;
        panel.classList.toggle('active', active);
        panel.hidden = !active;
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
    fetchBtn.textContent = 'Find My Titles';
    if (previewManualBtn) {
        previewManualBtn.textContent = 'Preview Selected Titles';
    }
}

function moveFrequencyTabFocus(currentTab, direction) {
    const tabs = Array.from(document.querySelectorAll('.tab'));
    const currentIndex = tabs.indexOf(currentTab);
    if (currentIndex === -1) return;

    const nextIndex = (currentIndex + direction + tabs.length) % tabs.length;
    const nextTab = tabs[nextIndex];
    switchFrequencyTab(nextTab.dataset.tab);
    nextTab.focus();
}

function handleFrequencyTabKeydown(event) {
    const tab = event.currentTarget;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
        event.preventDefault();
        moveFrequencyTabFocus(tab, -1);
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
        event.preventDefault();
        moveFrequencyTabFocus(tab, 1);
    } else if (event.key === 'Home') {
        event.preventDefault();
        const firstTab = document.querySelector('.tab');
        if (firstTab) {
            switchFrequencyTab(firstTab.dataset.tab);
            firstTab.focus();
        }
    } else if (event.key === 'End') {
        event.preventDefault();
        const tabs = document.querySelectorAll('.tab');
        const lastTab = tabs[tabs.length - 1];
        if (lastTab) {
            switchFrequencyTab(lastTab.dataset.tab);
            lastTab.focus();
        }
    }
}

function selectedDisplayMode() {
    return document.getElementById('displayMode')?.value || 'occurrence';
}

function selectedCombineMode() {
    return document.querySelector('input[name="combineMode"]:checked')?.value || 'average';
}

function appendFrequencyOptions(params) {
    params.set('display_mode', selectedDisplayMode());
    params.set('combine_mode', selectedCombineMode());
}

function selectedShelfStatuses(source) {
    return Array.from(document.querySelectorAll(`input[data-status-source="${source}"]:checked`))
        .map(input => input.value);
}

function appendShelfStatusParams(params) {
    const vndbStatuses = selectedShelfStatuses('vndb');
    const anilistStatuses = selectedShelfStatuses('anilist');
    if (vndbStatuses.join(',') !== DEFAULT_VNDB_STATUSES.join(',')) {
        params.set('vndb_status', vndbStatuses.join(','));
    }
    if (anilistStatuses.join(',') !== DEFAULT_ANILIST_STATUSES.join(',')) {
        params.set('anilist_status', anilistStatuses.join(','));
    }
}

function toggleShelfOptions() {
    const options = document.getElementById('shelfOptions');
    const toggle = document.getElementById('shelfToggle');
    const expanded = options.hidden;
    options.hidden = !expanded;
    toggle.setAttribute('aria-expanded', expanded ? 'true' : 'false');
}

function onShelfStatusChange(input) {
    const source = input.dataset.statusSource;
    const selected = selectedShelfStatuses(source);
    if (selected.length === 0) {
        input.checked = true;
    }
    input.closest('.shelf-chip')?.classList.toggle('active', input.checked);
    updateShelfSummary();
    updateIndexUrl();
}

function updateShelfSummary() {
    document.querySelectorAll('.shelf-chip').forEach(chip => {
        const input = chip.querySelector('input');
        chip.classList.toggle('active', Boolean(input?.checked));
    });

    const labels = Array.from(document.querySelectorAll('input[data-status-source]:checked'))
        .map(input => input.dataset.summaryLabel || statusLabel(input.value))
        .filter((label, index, all) => all.indexOf(label) === index);
    const summary = labels.length <= 2
        ? labels.join(', ')
        : `${labels.slice(0, 2).join(', ')} +${labels.length - 2}`;
    document.getElementById('shelfSummary').textContent = summary || 'Current titles';
}

function hasFrequencyMediaParams(params) {
    return params && (
        params.has('vndb_user') ||
        params.has('anilist_user') ||
        params.has('entries')
    );
}

function buildFrequencyParams({ validate = false } = {}) {
    const params = new URLSearchParams();

    if (activeMode() === 'manual') {
        if (validate && !validateManualEntries()) return null;
        const entries = getManualEntries();
        if (entries.length > 0) {
            params.set('entries', JSON.stringify(entries));
        }
        appendFrequencyOptions(params);
        return params;
    }

    if (validate && !validateUsernameInputs()) return null;
    const vndbUser = document.getElementById('vndbUser').value.trim();
    const anilistUser = document.getElementById('anilistUser').value.trim();

    if (vndbUser) params.set('vndb_user', vndbUser);
    if (anilistUser) params.set('anilist_user', anilistUser);
    appendShelfStatusParams(params);

    appendFrequencyOptions(params);
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

        renderMediaPreview(entries, 'Selected Titles');
        updateIndexUrl();
        setStatus(status, `Prepared ${entries.length} title${entries.length === 1 ? '' : 's'} for the Yomitan dictionary.`, 'success');
        return;
    }

    const params = buildFrequencyParams({ validate: true });
    if (!hasFrequencyMediaParams(params)) {
        setStatus(status, 'Please enter at least one VNDB or AniList username.', 'error');
        return;
    }

    fetchBtn.disabled = true;
    fetchBtn.textContent = 'Fetching...';
    preview.classList.remove('show');
    setStatus(status, 'Fetching selected VNDB/AniList titles...', 'loading');

    try {
        const response = await fetch('/api/user-lists?' + params.toString());
        const data = await response.json();
        if (!response.ok || data.error) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }

        renderMediaPreview(data.entries || [], 'Selected Titles');

        let message = `Found ${(data.entries || []).length} selected title${(data.entries || []).length === 1 ? '' : 's'}.`;
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

    if (!hasFrequencyMediaParams(params)) {
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
    setStatus(status, 'Building your Yomitan dictionary...', 'loading');

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
            setStatus(status, `Yomitan dictionary downloaded. Matched ${matched} title${matched === 1 ? '' : 's'} and combined ${terms} word/name entr${terms === 1 ? 'y' : 'ies'}.`, 'success');
        } catch (err) {
            setStatus(status, `Download error: ${err.message}`, 'error');
        } finally {
            generateBtn.disabled = false;
            setPreviewButtonsDisabled(false);
            generateBtn.textContent = 'Generate Yomitan Dictionary';
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
        generateBtn.textContent = 'Generate Yomitan Dictionary';
        hideProgress();
        updateActionButtons();
    });

    eventSource.onerror = () => {
        if (generateBtn.disabled) {
            eventSource.close();
            setStatus(status, 'Connection lost. Please try again.', 'error');
            generateBtn.disabled = false;
            setPreviewButtonsDisabled(false);
            generateBtn.textContent = 'Generate Yomitan Dictionary';
            hideProgress();
            updateActionButtons();
        }
    };
}

function attachManualMediaAutocomplete(row) {
    if (!window.BeeMediaAutocomplete) return;
    window.BeeMediaAutocomplete.attach(row, {
        validate: validateManualId,
        onChange: () => updateIndexUrl(),
    });
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
            <label>Title or ID</label>
            <input type="text" data-field="id" placeholder="e.g., Steins;Gate, v17, 9253, or https://anilist.co/anime/9253" oninput="validateManualId(this); updateIndexUrl();">
            <div class="input-hint"></div>
        </div>
        <button type="button" class="remove-entry-btn" onclick="removeManualEntry(this)" title="Remove entry">&times;</button>
    `;
    container.appendChild(row);
    attachManualMediaAutocomplete(row);
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
    if (window.BeeMediaAutocomplete) {
        window.BeeMediaAutocomplete.refresh(row);
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
    return Array.from(document.querySelectorAll('.manual-entry-row')).map(row => {
        const source = row.querySelector('[data-field="source"]').value;
        const idInput = row.querySelector('[data-field="id"]');
        const mediaType = row.querySelector('[data-field="media_type"]').value;
        const id = idInput.value.trim();
        if (!id) return null;

        return {
            source,
            id,
            title: idInput.dataset.mediaTitle || id,
            title_romaji: idInput.dataset.mediaTitleRomaji || idInput.dataset.mediaTitle || id,
            media_type: source === 'vndb'
                ? 'vn'
                : (mediaType || 'ANIME').toLowerCase(),
        };
    }).filter(Boolean);
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

function renderMediaPreview(entries, label = 'Selected Titles') {
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
            <div class="unmatched-reason">${escapeHtml(entry.reason || 'No matching word-count data found for this title')}</div>
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
    const shelf = entry.status
        ? `<span class="shelf-label">List: ${escapeHtml(statusLabel(entry.status))}</span>`
        : '';

    return `
        <span class="title">${escapeHtml(title)}</span>
        ${romaji}
        <span class="badge ${escapeHtml(badgeClass || '')}">${escapeHtml(badgeText)}</span>
        ${shelf}
    `;
}

function statusLabel(status) {
    return STATUS_LABELS[String(status || '').toLowerCase()] || 'Current';
}

function updateIndexUrl() {
    const params = buildFrequencyParams();
    lastIndexUrl = hasFrequencyMediaParams(params)
        ? `${location.origin}/api/yomitan-frequency-index?${params.toString()}`
        : '';
    const input = document.getElementById('indexUrl');
    input.value = lastIndexUrl;
    return lastIndexUrl;
}

function updateFrequencyPreview() {
    const valueEl = document.getElementById('frequencyPreviewValue');
    const countsEl = document.getElementById('frequencyPreviewCounts');
    if (!valueEl || !countsEl) return;

    const displayMode = selectedDisplayMode();
    const optionsEl = document.querySelector('.frequency-options');
    const combineGroupEl = document.getElementById('combineModeGroup');
    const hideCombine = displayMode === 'occurrence';
    if (optionsEl && combineGroupEl) {
        optionsEl.classList.toggle('combine-hidden', hideCombine);
        combineGroupEl.hidden = hideCombine;
    }
    const combineMode = selectedCombineMode();
    const totalOccurrences = SAMPLE_FREQUENCY_DECKS.reduce((sum, deck) => sum + deck.count, 0);
    const totalTokens = SAMPLE_FREQUENCY_DECKS.reduce((sum, deck) => sum + deck.total, 0);
    const averageRate = SAMPLE_FREQUENCY_DECKS.reduce((sum, deck) => {
        return sum + (deck.total ? deck.count / deck.total : 0);
    }, 0) / SAMPLE_FREQUENCY_DECKS.length;
    const sumRate = totalTokens ? totalOccurrences / totalTokens : 0;
    const selectedRate = combineMode === 'average' ? averageRate : sumRate;

    if (displayMode === 'occurrence') {
        valueEl.textContent = `${totalOccurrences}`;
    } else if (displayMode === 'per_million') {
        valueEl.textContent = `${formatPreviewNumber(selectedRate * 1000000)} / 1M (${combineModeLabel(combineMode)})`;
    } else if (displayMode === 'percent') {
        valueEl.textContent = `${formatPreviewNumber(selectedRate * 100)}% (${combineModeLabel(combineMode)})`;
    } else {
        valueEl.textContent = `#1 (${combineModeLabel(combineMode)})`;
    }

    countsEl.innerHTML = SAMPLE_FREQUENCY_DECKS.map(deck => `
        <span>${escapeHtml(deck.title)}: 秋 ${deck.count} / ${deck.total.toLocaleString()}</span>
    `).join('');
}

function combineModeLabel(mode) {
    return mode === 'average' ? 'per title' : 'total';
}

function formatPreviewNumber(value) {
    const fixed = value.toFixed(2);
    if (fixed.endsWith('.00')) {
        return fixed.slice(0, -3);
    }
    if (fixed.endsWith('0')) {
        return fixed.slice(0, -1);
    }
    return fixed;
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
        setStatus(status, 'Generate a Yomitan URL first.', 'error');
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
        setHint(hint, input, 'Choose a VNDB suggestion, or enter a VN ID like <b>v17</b>, <b>17</b>, or a vndb.org URL.', 'error');
        return false;
    }

    if (/^\d+$/.test(val)) {
        clearHint(hint, input);
        return true;
    }

    setHint(hint, input, 'Choose an AniList suggestion, or enter a numeric AniList ID like <b>9253</b> or an anilist.co URL.', 'error');
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
        tab.addEventListener('keydown', handleFrequencyTabKeydown);
    });
    addManualEntry();
    updateActionButtons();
    updateShelfSummary();
    updateFrequencyPreview();
    updateIndexUrl();
});
