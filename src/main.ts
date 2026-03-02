import { invoke, addPluginListener } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import type { AppConfig } from './types';

// Listen for notification clicks to reveal files in Explorer
addPluginListener('notification', 'actionPerformed', (event: any) => {
  const filepath = event?.extra?.filepath ?? event?.notification?.extra?.filepath;
  if (filepath) {
    invoke('reveal_file', { path: filepath }).catch(() => {});
  }
}).catch(() => {});

let folderInput: HTMLInputElement;
let formatSelect: HTMLSelectElement;
let prefixInput: HTMLInputElement;
let suffixInput: HTMLInputElement;
let copyToClipboardCheckbox: HTMLInputElement;
let saveToDiskCheckbox: HTMLInputElement;
let captureModeSelect: HTMLSelectElement;
let annotateCapturesCheckbox: HTMLInputElement;
let modDefaultSelect: HTMLSelectElement;
let modShiftSelect: HTMLSelectElement;
let modCtrlSelect: HTMLSelectElement;
let modAltSelect: HTMLSelectElement;
let launchOnStartupCheckbox: HTMLInputElement;
let folderWarning: HTMLSpanElement;

// -- Hotkey recorder --

interface HotkeyState {
  input: HTMLInputElement;
  rawValue: string; // internal format: "CmdOrCtrl+Alt+Shift+S"
  recording: boolean;
}

const isMac = navigator.platform.toUpperCase().includes('MAC');

function formatHotkeyDisplay(raw: string): string {
  return raw
    .replace(/CmdOrCtrl/g, isMac ? 'Cmd' : 'Ctrl')
    .split('+')
    .map(p => p.charAt(0).toUpperCase() + p.slice(1))
    .join(' + ');
}

function initHotkeyRecorder(input: HTMLInputElement, initialValue: string, onComplete?: () => void): HotkeyState {
  const state: HotkeyState = { input, rawValue: initialValue, recording: false };
  input.value = formatHotkeyDisplay(initialValue);

  input.addEventListener('click', () => {
    state.recording = true;
    input.value = 'Press a key combination...';
    input.classList.add('recording');
  });

  input.addEventListener('blur', () => {
    if (state.recording) {
      state.recording = false;
      input.value = formatHotkeyDisplay(state.rawValue);
      input.classList.remove('recording');
    }
  });

  input.addEventListener('keydown', (e: KeyboardEvent) => {
    if (!state.recording) return;
    e.preventDefault();
    e.stopPropagation();

    // Ignore standalone modifier presses
    if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return;

    // Escape cancels
    if (e.key === 'Escape') {
      state.recording = false;
      input.value = formatHotkeyDisplay(state.rawValue);
      input.classList.remove('recording');
      input.blur();
      return;
    }

    const parts: string[] = [];
    if (e.ctrlKey || e.metaKey) parts.push('CmdOrCtrl');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');

    // Require at least one modifier
    if (parts.length === 0) return;

    // Convert key to Tauri format
    const keyStr = keyToTauriFormat(e.key);
    if (!keyStr) return;

    parts.push(keyStr);
    state.rawValue = parts.join('+');
    state.recording = false;
    input.value = formatHotkeyDisplay(state.rawValue);
    input.classList.remove('recording');
    input.blur();
    onComplete?.();
  });

  return state;
}

function keyToTauriFormat(key: string): string | null {
  // Letter keys
  if (key.length === 1 && /[a-zA-Z]/.test(key)) {
    return key.toUpperCase();
  }
  // Number keys
  if (key.length === 1 && /[0-9]/.test(key)) {
    return key;
  }
  // Function keys
  if (/^F\d+$/.test(key)) return key;

  // Special keys
  const specialMap: Record<string, string> = {
    ' ': 'Space', 'Tab': 'Tab', 'Enter': 'Enter',
    'Backspace': 'Backspace', 'Delete': 'Delete',
    'Home': 'Home', 'End': 'End',
    'PageUp': 'PageUp', 'PageDown': 'PageDown',
    'ArrowUp': 'Up', 'ArrowDown': 'Down',
    'ArrowLeft': 'Left', 'ArrowRight': 'Right',
    'Insert': 'Insert',
  };
  return specialMap[key] || null;
}

// -- Save folder validation --

async function validateFolder() {
  const folder = folderInput.value.trim();
  if (!folder) {
    folderWarning.style.display = 'none';
    return;
  }
  try {
    const result: string = await invoke('validate_save_folder', { folder });
    if (result === 'ok') {
      folderWarning.style.display = 'none';
    } else {
      folderWarning.textContent = result;
      folderWarning.style.display = 'block';
    }
  } catch {
    folderWarning.style.display = 'none';
  }
}

// -- Auto-save config --

let regionHotkey: HotkeyState;
let fullscreenHotkey: HotkeyState;
let windowHotkey: HotkeyState;
let recordHotkey: HotkeyState;
let recordingFormatSelect: HTMLSelectElement;
let recordingFpsSelect: HTMLSelectElement;
let gifMaxDurationInput: HTMLInputElement;
let gifMaxWidthInput: HTMLInputElement;

let lastSavedConfig: AppConfig | null = null;

function collectConfig(): AppConfig {
  return {
    save_folder: folderInput.value,
    hotkey_region: regionHotkey.rawValue,
    hotkey_fullscreen: fullscreenHotkey.rawValue,
    hotkey_window: windowHotkey.rawValue,
    format: formatSelect.value as AppConfig['format'],
    filename_prefix: prefixInput.value,
    filename_suffix: suffixInput.value,
    copy_to_clipboard: copyToClipboardCheckbox.checked,
    save_to_disk: saveToDiskCheckbox.checked,
    capture_mode: captureModeSelect.value as AppConfig['capture_mode'],
    annotate_captures: annotateCapturesCheckbox.checked,
    annotate_default_tool: modDefaultSelect.value,
    annotate_shift_tool: modShiftSelect.value,
    annotate_ctrl_tool: modCtrlSelect.value,
    annotate_alt_tool: modAltSelect.value,
    launch_on_startup: launchOnStartupCheckbox.checked,
    hotkey_record: recordHotkey.rawValue,
    recording_format: recordingFormatSelect.value as AppConfig['recording_format'],
    recording_fps: parseInt(recordingFpsSelect.value, 10),
    gif_max_duration: parseInt(gifMaxDurationInput.value, 10) || 15,
    gif_max_width: parseInt(gifMaxWidthInput.value, 10) || 800,
  };
}

function applyConfig(config: AppConfig) {
  folderInput.value = config.save_folder;
  regionHotkey.rawValue = config.hotkey_region;
  regionHotkey.input.value = formatHotkeyDisplay(config.hotkey_region);
  fullscreenHotkey.rawValue = config.hotkey_fullscreen;
  fullscreenHotkey.input.value = formatHotkeyDisplay(config.hotkey_fullscreen);
  windowHotkey.rawValue = config.hotkey_window;
  windowHotkey.input.value = formatHotkeyDisplay(config.hotkey_window);
  formatSelect.value = config.format;
  prefixInput.value = config.filename_prefix;
  suffixInput.value = config.filename_suffix;
  copyToClipboardCheckbox.checked = config.copy_to_clipboard;
  saveToDiskCheckbox.checked = config.save_to_disk;
  captureModeSelect.value = config.capture_mode;
  annotateCapturesCheckbox.checked = config.annotate_captures;
  modDefaultSelect.value = config.annotate_default_tool;
  modShiftSelect.value = config.annotate_shift_tool;
  modCtrlSelect.value = config.annotate_ctrl_tool;
  modAltSelect.value = config.annotate_alt_tool;
  launchOnStartupCheckbox.checked = config.launch_on_startup;
  recordHotkey.rawValue = config.hotkey_record;
  recordHotkey.input.value = formatHotkeyDisplay(config.hotkey_record);
  recordingFormatSelect.value = config.recording_format;
  recordingFpsSelect.value = String(config.recording_fps);
  gifMaxDurationInput.value = String(config.gif_max_duration);
  gifMaxWidthInput.value = String(config.gif_max_width);
}

async function autoSave() {
  const config = collectConfig();
  const saveError = document.getElementById('save-error')!;
  try {
    await invoke('save_config', { newConfig: config });
    lastSavedConfig = config;
    saveError.style.display = 'none';
  } catch (e) {
    saveError.textContent = String(e);
    saveError.style.display = 'block';
    // Revert UI to last known good config so the user sees what's actually persisted.
    // Programmatic .value/.checked sets don't fire change events, so no recursive save.
    if (lastSavedConfig) applyConfig(lastSavedConfig);
  }
}


// -- Init --

async function loadConfig() {
  const config: AppConfig = await invoke('get_config');
  applyConfig(config);
  lastSavedConfig = config;
  validateFolder();
}

async function browseSaveFolder() {
  const folder: string | null = await invoke('pick_folder');
  if (folder) {
    folderInput.value = folder;
    validateFolder();
    autoSave();
  }
}

window.addEventListener('DOMContentLoaded', () => {
  folderInput = document.getElementById('save-folder') as HTMLInputElement;
  const regionHotkeyInput = document.getElementById('hotkey-region') as HTMLInputElement;
  const fullscreenHotkeyInput = document.getElementById('hotkey-fullscreen') as HTMLInputElement;
  const windowHotkeyInput = document.getElementById('hotkey-window') as HTMLInputElement;
  formatSelect = document.getElementById('format') as HTMLSelectElement;
  prefixInput = document.getElementById('filename-prefix') as HTMLInputElement;
  suffixInput = document.getElementById('filename-suffix') as HTMLInputElement;
  copyToClipboardCheckbox = document.getElementById('copy-to-clipboard') as HTMLInputElement;
  saveToDiskCheckbox = document.getElementById('save-to-disk') as HTMLInputElement;
  captureModeSelect = document.getElementById('capture-mode') as HTMLSelectElement;
  annotateCapturesCheckbox = document.getElementById('annotate-captures') as HTMLInputElement;
  modDefaultSelect = document.getElementById('mod-default') as HTMLSelectElement;
  modShiftSelect = document.getElementById('mod-shift') as HTMLSelectElement;
  modCtrlSelect = document.getElementById('mod-ctrl') as HTMLSelectElement;
  modAltSelect = document.getElementById('mod-alt') as HTMLSelectElement;
  launchOnStartupCheckbox = document.getElementById('launch-on-startup') as HTMLInputElement;
  folderWarning = document.getElementById('folder-warning') as HTMLSpanElement;
  const recordHotkeyInput = document.getElementById('hotkey-record') as HTMLInputElement;
  recordingFormatSelect = document.getElementById('recording-format') as HTMLSelectElement;
  recordingFpsSelect = document.getElementById('recording-fps') as HTMLSelectElement;
  gifMaxDurationInput = document.getElementById('gif-max-duration') as HTMLInputElement;
  gifMaxWidthInput = document.getElementById('gif-max-width') as HTMLInputElement;

  // Init hotkey recorders -- autoSave on recording complete
  regionHotkey = initHotkeyRecorder(regionHotkeyInput, '', autoSave);
  fullscreenHotkey = initHotkeyRecorder(fullscreenHotkeyInput, '', autoSave);
  windowHotkey = initHotkeyRecorder(windowHotkeyInput, '', autoSave);
  recordHotkey = initHotkeyRecorder(recordHotkeyInput, '', autoSave);

  // Checkboxes and selects: save immediately on change
  const immediateElements = [
    formatSelect, captureModeSelect, copyToClipboardCheckbox, saveToDiskCheckbox,
    annotateCapturesCheckbox, launchOnStartupCheckbox,
    modDefaultSelect, modShiftSelect, modCtrlSelect, modAltSelect,
    recordingFormatSelect, recordingFpsSelect,
  ];
  for (const el of immediateElements) {
    el.addEventListener('change', autoSave);
  }

  // Text/number inputs: save on blur only (change event) to avoid reverting
  // the field mid-typing when an intermediate value fails validation
  // (e.g., typing "%Y-" before finishing "%Y-%m-%d_%H-%M-%S").
  const textElements = [folderInput, prefixInput, suffixInput, gifMaxDurationInput, gifMaxWidthInput];
  for (const el of textElements) {
    el.addEventListener('change', autoSave);
  }

  // Folder validation on edit (debounced separately for the warning display)
  let folderValidateTimer: ReturnType<typeof setTimeout>;
  folderInput.addEventListener('input', () => {
    clearTimeout(folderValidateTimer);
    folderValidateTimer = setTimeout(validateFolder, 300);
  });
  folderInput.addEventListener('change', validateFolder);

  document.getElementById('browse-btn')!.addEventListener('click', browseSaveFolder);

  // Tab switching
  const tabs = document.querySelectorAll<HTMLButtonElement>('.tab-bar .tab');
  const panels = document.querySelectorAll<HTMLDivElement>('.tab-panel');
  const saveError = document.getElementById('save-error')!;

  tabs.forEach(tab => {
    tab.addEventListener('click', () => {
      const target = tab.dataset.tab!;
      tabs.forEach(t => t.classList.toggle('active', t === tab));
      panels.forEach(p => p.classList.toggle('active', p.dataset.tab === target));
      if (target === 'about') saveError.style.display = 'none';
    });
  });

  // About section
  getVersion().then(v => {
    document.getElementById('about-version')!.textContent = `v${v}`;
  });

  // Update checker
  const checkUpdateBtn = document.getElementById('check-update-btn') as HTMLButtonElement;
  const updateStatus = document.getElementById('update-status')!;
  const restartBtn = document.getElementById('restart-btn') as HTMLButtonElement;

  checkUpdateBtn.addEventListener('click', async () => {
    checkUpdateBtn.disabled = true;
    updateStatus.className = 'update-status';
    updateStatus.textContent = 'Checking for updates...';

    try {
      const update = await check();
      if (!update) {
        updateStatus.textContent = 'You are on the latest version.';
        updateStatus.classList.add('success');
        checkUpdateBtn.disabled = false;
        return;
      }

      updateStatus.textContent = `Downloading v${update.version}...`;

      let totalBytes = 0;
      let receivedBytes = 0;
      await update.downloadAndInstall((progress) => {
        if (progress.event === 'Started' && progress.data.contentLength) {
          totalBytes = progress.data.contentLength;
        } else if (progress.event === 'Progress') {
          receivedBytes += progress.data.chunkLength;
          if (totalBytes > 0) {
            const pct = Math.round((receivedBytes / totalBytes) * 100);
            updateStatus.textContent = `Downloading v${update.version}... ${pct}%`;
          }
        } else if (progress.event === 'Finished') {
          updateStatus.textContent = 'Update installed. Restart to apply.';
          updateStatus.classList.add('success');
        }
      });

      restartBtn.style.display = '';
    } catch (e) {
      updateStatus.textContent = `Update failed: ${e}`;
      updateStatus.classList.add('error');
      checkUpdateBtn.disabled = false;
    }
  });

  restartBtn.addEventListener('click', async () => {
    await relaunch();
  });

  loadConfig();
});
