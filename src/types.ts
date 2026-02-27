export interface AppConfig {
  save_folder: string;
  hotkey_region: string;
  hotkey_fullscreen: string;
  format: 'jpg' | 'png' | 'webp';
  filename_prefix: string;
  save_to_disk: boolean;
}

export interface CaptureResult {
  filepath: string | null;
  copied_to_clipboard: boolean;
}
