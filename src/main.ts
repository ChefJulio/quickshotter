import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { AppConfig } from './types';

let folderInput: HTMLInputElement;
let formatSelect: HTMLSelectElement;
let prefixInput: HTMLInputElement;
let suffixInput: HTMLInputElement;
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

function initHotkeyRecorder(input: HTMLInputElement, initialValue: string): HotkeyState {
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

// -- Config load/save --

let regionHotkey: HotkeyState;
let fullscreenHotkey: HotkeyState;
let windowHotkey: HotkeyState;

async function loadConfig() {
  const config: AppConfig = await invoke('get_config');
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
  saveToDiskCheckbox.checked = config.save_to_disk;
  captureModeSelect.value = config.capture_mode;
  annotateCapturesCheckbox.checked = config.annotate_captures;
  modDefaultSelect.value = config.annotate_default_tool;
  modShiftSelect.value = config.annotate_shift_tool;
  modCtrlSelect.value = config.annotate_ctrl_tool;
  modAltSelect.value = config.annotate_alt_tool;
  launchOnStartupCheckbox.checked = config.launch_on_startup;
  validateFolder();
}

async function saveConfig() {
  const config: AppConfig = {
    save_folder: folderInput.value,
    hotkey_region: regionHotkey.rawValue,
    hotkey_fullscreen: fullscreenHotkey.rawValue,
    hotkey_window: windowHotkey.rawValue,
    format: formatSelect.value as AppConfig['format'],
    filename_prefix: prefixInput.value,
    filename_suffix: suffixInput.value,
    save_to_disk: saveToDiskCheckbox.checked,
    capture_mode: captureModeSelect.value as AppConfig['capture_mode'],
    annotate_captures: annotateCapturesCheckbox.checked,
    annotate_default_tool: modDefaultSelect.value,
    annotate_shift_tool: modShiftSelect.value,
    annotate_ctrl_tool: modCtrlSelect.value,
    annotate_alt_tool: modAltSelect.value,
    launch_on_startup: launchOnStartupCheckbox.checked,
  };

  const saveError = document.getElementById('save-error')!;
  try {
    saveError.style.display = 'none';
    await invoke('save_config', { newConfig: config });
    getCurrentWindow().close();
  } catch (e) {
    saveError.textContent = `Failed to save: ${e}`;
    saveError.style.display = 'block';
  }
}

async function browseSaveFolder() {
  const folder: string | null = await invoke('pick_folder');
  if (folder) {
    folderInput.value = folder;
    validateFolder();
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
  saveToDiskCheckbox = document.getElementById('save-to-disk') as HTMLInputElement;
  captureModeSelect = document.getElementById('capture-mode') as HTMLSelectElement;
  annotateCapturesCheckbox = document.getElementById('annotate-captures') as HTMLInputElement;
  modDefaultSelect = document.getElementById('mod-default') as HTMLSelectElement;
  modShiftSelect = document.getElementById('mod-shift') as HTMLSelectElement;
  modCtrlSelect = document.getElementById('mod-ctrl') as HTMLSelectElement;
  modAltSelect = document.getElementById('mod-alt') as HTMLSelectElement;
  launchOnStartupCheckbox = document.getElementById('launch-on-startup') as HTMLInputElement;
  folderWarning = document.getElementById('folder-warning') as HTMLSpanElement;

  // Init hotkey recorders
  regionHotkey = initHotkeyRecorder(regionHotkeyInput, '');
  fullscreenHotkey = initHotkeyRecorder(fullscreenHotkeyInput, '');
  windowHotkey = initHotkeyRecorder(windowHotkeyInput, '');

  // Folder validation on edit -- debounced to avoid excessive IPC calls while typing
  let folderValidateTimer: ReturnType<typeof setTimeout>;
  folderInput.addEventListener('input', () => {
    clearTimeout(folderValidateTimer);
    folderValidateTimer = setTimeout(validateFolder, 300);
  });
  folderInput.addEventListener('change', validateFolder);

  document.getElementById('browse-btn')!.addEventListener('click', browseSaveFolder);
  document.getElementById('save-btn')!.addEventListener('click', saveConfig);
  document.getElementById('cancel-btn')!.addEventListener('click', () => getCurrentWindow().close());

  loadConfig();
});
