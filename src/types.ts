export interface AppConfig {
  save_folder: string;
  hotkey_region: string;
  hotkey_fullscreen: string;
  hotkey_window: string;
  format: 'jpg' | 'png' | 'webp';
  filename_prefix: string;
  save_to_disk: boolean;
  clipboard_action: 'image' | 'url' | 'none';
  capture_mode: 'instant' | 'freeze' | 'window';
  annotate_captures: boolean;
  annotate_shift_tool: string;
  annotate_ctrl_tool: string;
  annotate_alt_tool: string;
  annotate_default_tool: string;
  annotate_right_default_tool: string;
  annotate_right_shift_tool: string;
  annotate_right_ctrl_tool: string;
  annotate_right_alt_tool: string;
  filename_suffix: string;
  launch_on_startup: boolean;
  explorer_context_menu: boolean;
  fullscreen_mode: 'all' | 'current' | 'select';
  capture_delay: number;
  hotkey_ocr: string;
  hotkey_record: string;
  recording_format: 'mp4' | 'gif';
  recording_fps: number;
  gif_max_duration: number;
  gif_max_width: number;
  recording_prefix: string;
}

export interface CaptureResult {
  filepath: string | null;
  copied_to_clipboard: boolean;
}

export interface WindowRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface AnnotationConfig {
  shift_tool: string;
  ctrl_tool: string;
  alt_tool: string;
  default_tool: string;
  right_default_tool: string;
  right_shift_tool: string;
  right_ctrl_tool: string;
  right_alt_tool: string;
}
