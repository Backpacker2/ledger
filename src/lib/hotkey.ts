// Shortcuts are stored the way Tauri's global-shortcut plugin reads them: "Control+Shift+S", "Alt+F9",
// "CommandOrControl+Shift+S". Keys are named by the physical key (`KeyboardEvent.code`), so the shortcut means the
// same on every keyboard layout.

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'OS']);

const platform = () => (typeof navigator === 'undefined' ? '' : navigator.platform.toLowerCase());
const onMac = () => platform().includes('mac');

/** The shortcut a key press spells, or null while only modifiers are held down. */
export function accelerator(event: KeyboardEvent): string | null {
	if (MODIFIER_KEYS.has(event.key)) return null;
	const parts: string[] = [];
	if (event.ctrlKey) parts.push('Control');
	if (event.altKey) parts.push('Alt');
	if (event.shiftKey) parts.push('Shift');
	if (event.metaKey) parts.push('Super');
	const key = keyName(event);
	if (!key) return null;
	parts.push(key);
	return parts.join('+');
}

/**
 * "KeyS" → "S", "Digit5" → "5"; other codes (F5, Space, ArrowUp, PrintScreen) are already what the plugin reads. Input
 * that carries no scan code (remote desktops, virtual keyboards) has an empty `code`; the character is used then.
 */
function keyName(event: KeyboardEvent): string | null {
	const { code, key } = event;
	if (!code) return key === 'Unidentified' || key === 'Dead' ? null : key.length === 1 ? key.toUpperCase() : key;
	if (/^Key[A-Z]$/.test(code)) return code.slice(3);
	if (/^Digit\d$/.test(code)) return code.slice(5);
	return code;
}

/** A shortcut as this platform writes it: "Ctrl + Shift + S", or "⌘⇧S" on a Mac. */
export function prettyHotkey(accel: string): string {
	const mac = onMac();
	const names = accel.split('+').map((part) => {
		switch (part.toLowerCase()) {
			case 'commandorcontrol':
			case 'commandorctrl':
			case 'cmdorcontrol':
			case 'cmdorctrl':
				return mac ? '⌘' : 'Ctrl';
			case 'control':
			case 'ctrl':
				return mac ? '⌃' : 'Ctrl';
			case 'alt':
			case 'option':
				return mac ? '⌥' : 'Alt';
			case 'shift':
				return mac ? '⇧' : 'Shift';
			case 'super':
			case 'command':
			case 'cmd':
				return mac ? '⌘' : platform().includes('win') ? 'Win' : 'Super';
			default:
				return part.replace(/^Arrow/, '');
		}
	});
	return names.join(mac ? '' : ' + ');
}
