// Lightweight i18n: English source strings → Japanese translations.
// Language is detected once from navigator.language at module load.
// `t(key)` returns the original key when language is English (no lookup needed),
// or the translation when Japanese — so call sites stay readable.

const ja: Record<string, string> = {
	// Navigation
	'Live': 'ライブ',
	'Media': 'メディア',
	'Mask': 'マスク',
	'Solve': 'ソルブ',
	'Settings': '設定',
	'Switch to light mode': 'ライトモードに切替',
	'Switch to dark mode': 'ダークモードに切替',

	// Connection status
	'Connected': '接続済み',
	'Disconnected': '切断',
	'Connecting…': '接続中…',

	// Settings tabs
	'System': 'システム',
	'Detection': '検出',
	'Record': '録画',
	'Webhook': 'Webhook',
	'Maintenance': 'メンテナンス',
	'Daily auto-reboot': '毎日の自動再起動',
	'Reboot time': '再起動時刻',
	'Auto': '自動',
	'On': 'ON',
	'Off': 'OFF',
	'Day': '昼',
	'Night': '夜',
	'Day / Night': '昼夜モード',
	'Image': '画像処理',
	'Image Processing': '画像処理',
	'Toggle ISP modules. Settings persist across reboot.':
		'ISPモジュールの ON/OFF。設定は再起動後も保持されます。',
	'Reading current ISP state…': '現在のISP状態を取得中…',
	'★ meteor: off': '★ 流星: OFF推奨',
	'★ meteor: on':  '★ 流星: ON推奨',

	// Section headings
	'Camera': 'カメラ',
	'Time': '時刻',
	'Location': '位置情報',
	'OSD': 'OSD',
	'Endpoints': 'エンドポイント',
	'Notifications': '通知',
	'Regular Recording': '定常録画',
	'Timelapse': 'タイムラプス',
	'Detection Recording': '検出録画',
	'View': '表示',
	'Functions': '機能',
	'Webhooks': 'Webhook',

	// Camera labels
	'Auto Day/Night': '昼夜自動',
	'Auto Day / Night': '昼夜自動',
	'Night Mode': '夜間モード',
	'IR Cut Filter': '赤外線フィルタ',
	'IR Cut': '赤外線カット',
	'IR LED': '赤外線LED',

	// Time / Location
	'Timezone': 'タイムゾーン',
	'Used for civil-twilight scheduling. North/East positive.':
		'市民薄明スケジュールに使用。北・東を正の値で。',
	'Latitude': '緯度',
	'Longitude': '経度',

	// OSD
	'Show Timestamp': 'タイムスタンプ表示',
	'Timestamp': 'タイムスタンプ',
	'Timestamp Position': '表示位置',
	'Position': '位置',
	'Top Left': '左上',
	'Top Right': '右上',
	'Bottom Left': '左下',
	'Bottom Right': '右下',
	'Top-left': '左上',
	'Top-right': '右上',
	'Bottom-left': '左下',
	'Bottom-right': '右下',

	// Detection / record / timelapse / schedule
	'Enable': '有効',
	'Enable Detection': '検出を有効化',
	'Plate Solver': 'プレートソルバー',
	'Camera moved?': 'カメラを動かした?',
	'Calibrated': '校正済み',
	'verified': '検証',
	'ago': '前',
	'Calibrating': '校正中',
	'Verifying': '検証中',
	'waiting for night': '夜を待機中',
	'retry in': '再試行まで',
	's': '秒',
	'min': '分',
	'h': '時間',
	'Recalibrate': '再キャリブレーション',
	'Discard the saved solution and calibrate again?': '保存済みの解を破棄して、もう一度キャリブレーションしますか?',
	'Enable Plate Solver': 'プレートソルバーを有効化',
	'Mode': 'モード',
	'By hours': '時間指定',
	'Civil twilight (night)': '市民薄明 (夜間)',
	'Twilight': '薄明',
	'Civil': '市民薄明',
	'Astro': '天文薄明',
	'Astronomical twilight (deep night)': '天文薄明 (深夜)',
	'⚠ Location is default (Tokyo). Configure in Settings.':
		'⚠ 位置情報が初期値 (東京) です。設定画面で登録してください。',
	'Schedule': 'スケジュール',
	'Recording': '録画',
	'Detection Record': '検出録画',
	'Hours': '時間',

	// Webhook
	'Generic': '汎用',
	'Startup': '起動時',
	'Detection Start': '検出開始',
	'Detection Stop': '検出終了',
	'Notify on startup': '起動時に通知',
	'Notify detection start': '検出開始を通知',
	'Notify detection end': '検出終了を通知',

	// Sidebar misc
	'Debug overlay': 'デバッグ表示',
	'Star overlay': '恒星表示',
	'Constellation overlay': '星座表示',
	'No plate solution': 'プレートソルブ結果なし',
	'HISTOGRAM': 'ヒストグラム',
	'Capture FITS': 'FITSスナップショット',
	'Capture': '撮影',
	'Reboot': '再起動',
	'Reboot the device?': 'デバイスを再起動しますか?',

	// Media page
	'Records': '録画',
	'Detections': '検出',
	'Refresh': '更新',
	'Loading…': '読み込み中…',
	'No files in': '次の場所にファイルなし:',
	'files': '件',

	// Mask page
	'Mask Editor': 'マスクエディタ',
	'cells masked': 'セルをマスク中',
	'Brush': 'ブラシ',
	'Pan': '移動',
	'Zoom': 'ズーム',
	'Undo': '元に戻す',
	'Erase': '消去',
	'Clear': 'クリア',
	'Fill All': '全選択',
	'Save': '保存',
	'Saving...': '保存中…',
	'Click/drag to mask. Right-click/Ctrl+click or Erase mode to unmask.':
		'クリック/ドラッグでマスク。右クリック・Ctrl+クリックまたは消去モードでマスク解除。ズーム時は「移動」でスクロール。',

	// Solve page
	'Star Editor': '恒星エディタ',
	'stars': '個の恒星',
	'Capturing…': '撮影中…',
	'Solving…': 'ソルブ中…',
	'Need at least 4 stars': '恒星が4個以上必要です',
	'Capture a frame, then click empty space to add a star or click a marker to remove it. Need ≥ 4 stars to solve.':
		'フレームを撮影し、空白をクリックで恒星を追加、マーカーをクリックで削除。ソルブには4個以上必要です。',
	'Press Capture to grab a frame': '「撮影」を押してフレームを取得',
	'No solution found': '解が見つかりませんでした',

	// Logs
	'Log': 'ログ',
	'Meteors': '流星',
	'FF Blocks': 'FFブロック',
	'No detections yet': '検出なし',
	'No FF blocks saved yet': '保存されたFFブロックはありません',
	'clear': 'クリア',
	'View stack image': 'スタック画像を表示',
	'Download FITS': 'FITSをダウンロード',
};

function detectLang(): 'en' | 'ja' {
	if (typeof navigator === 'undefined') return 'en';
	const l = (navigator.language ?? '').toLowerCase();
	return l.startsWith('ja') ? 'ja' : 'en';
}

export const lang: 'en' | 'ja' = detectLang();

export function t(key: string): string {
	return lang === 'ja' ? (ja[key] ?? key) : key;
}
