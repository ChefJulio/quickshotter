import { invoke } from '@tauri-apps/api/core';
import type { AppConfig } from './types';

let folderInput: HTMLInputElement;
let regionHotkeyInput: HTMLInputElement;
let fullscreenHotkeyInput: HTMLInputElement;
let formatSelect: HTMLSelectElement;
let prefixInput: HTMLInputElement;
let saveToDiskCheckbox: HTMLInputElement;

async function loadConfig() {
  const config: AppConfig = await invoke('get_config');
  folderInput.value = config.save_folder;
  regionHotkeyInput.value = config.hotkey_region;
  fullscreenHotkeyInput.value = config.hotkey_fullscreen;
  formatSelect.value = config.format;
  prefixInput.value = config.filename_prefix;
  saveToDiskCheckbox.checked = config.save_to_disk;
}

async function saveConfig() {
  const config: AppConfig = {
    save_folder: folderInput.value,
    hotkey_region: regionHotkeyInput.value,
    hotkey_fullscreen: fullscreenHotkeyInput.value,
    format: formatSelect.value as AppConfig['format'],
    filename_prefix: prefixInput.value,
    save_to_disk: saveToDiskCheckbox.checked,
  };

  try {
    await invoke('save_config', { newConfig: config });
    window.close();
  } catch (e) {
    alert(`Failed to save settings: ${e}`);
  }
}

async function browseSaveFolder() {
  const folder: string | null = await invoke('pick_folder');
  if (folder) {
    folderInput.value = folder;
  }
}

window.addEventListener('DOMContentLoaded', () => {
  folderInput = document.getElementById('save-folder') as HTMLInputElement;
  regionHotkeyInput = document.getElementById('hotkey-region') as HTMLInputElement;
  fullscreenHotkeyInput = document.getElementById('hotkey-fullscreen') as HTMLInputElement;
  formatSelect = document.getElementById('format') as HTMLSelectElement;
  prefixInput = document.getElementById('filename-prefix') as HTMLInputElement;
  saveToDiskCheckbox = document.getElementById('save-to-disk') as HTMLInputElement;

  document.getElementById('browse-btn')!.addEventListener('click', browseSaveFolder);
  document.getElementById('save-btn')!.addEventListener('click', saveConfig);
  document.getElementById('cancel-btn')!.addEventListener('click', () => window.close());

  loadConfig();
});
