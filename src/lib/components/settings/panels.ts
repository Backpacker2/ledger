// Every settings panel, in the group (the tab) it belongs to. The tabs show their group's panels from this list,
// and the search looks through all of it, so a new panel is added here once and is found both ways.

import type { Component } from 'svelte';
import Sparkles from '@lucide/svelte/icons/sparkles';
import Palette from '@lucide/svelte/icons/palette';
import RefreshCw from '@lucide/svelte/icons/refresh-cw';
import Wrench from '@lucide/svelte/icons/wrench';
import Info from '@lucide/svelte/icons/info';
import ScrollText from '@lucide/svelte/icons/scroll-text';
import AboutSettings from './AboutSettings.svelte';
import CaptureSettings from './CaptureSettings.svelte';
import ClaudeCodeSettings from './ClaudeCodeSettings.svelte';
import DashboardSettings from './DashboardSettings.svelte';
import EncryptionSettings from './EncryptionSettings.svelte';
import GitHubSettings from './GitHubSettings.svelte';
import LogSettings from './LogSettings.svelte';
import OrderSettings from './OrderSettings.svelte';
import ScreenshotSettings from './ScreenshotSettings.svelte';
import ThemeSettings from './ThemeSettings.svelte';
import TrelloSettings from './TrelloSettings.svelte';
import UpdateSettings from './UpdateSettings.svelte';
import VaultSettings from './VaultSettings.svelte';
import YellSettings from './YellSettings.svelte';

export type GroupId = 'features' | 'visual' | 'sync' | 'tools' | 'about' | 'logs';

export const groups: { id: GroupId; label: string; icon: Component }[] = [
	{ id: 'features', label: 'Features', icon: Sparkles },
	{ id: 'visual', label: 'Visual', icon: Palette },
	{ id: 'sync', label: 'Sync & external', icon: RefreshCw },
	{ id: 'tools', label: 'Tools', icon: Wrench },
	{ id: 'about', label: 'About', icon: Info },
	{ id: 'logs', label: 'Logs', icon: ScrollText }
];

export interface Panel {
	id: string;
	group: GroupId;
	title: string;
	/** Words the search also finds the panel by: what is on it, and what people might call it. */
	keywords: string;
	/** Every panel is given the settings; the ones that don't need them ignore them. */
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	component: Component<any>;
}

export const panels: Panel[] = [
	{ id: 'dashboard', group: 'features', title: 'Contribution graph', keywords: 'dashboard home activity heatmap squares days', component: DashboardSettings },
	{ id: 'theme', group: 'visual', title: 'Theme', keywords: 'colours colors dark light night appearance look accent', component: ThemeSettings },
	{ id: 'order', group: 'visual', title: 'Order', keywords: 'sort newest oldest first lists posts threads feedback versions', component: OrderSettings },
	{ id: 'vault', group: 'sync', title: 'Vault', keywords: 'folder location path obsidian markdown reload disk files switch', component: VaultSettings },
	{ id: 'github', group: 'sync', title: 'Sync to GitHub', keywords: 'repository repo branch access token private push pull status sync now', component: GitHubSettings },
	{ id: 'encryption', group: 'sync', title: 'Encryption', keywords: 'encrypt key recovery aes secret import export generate', component: EncryptionSettings },
	{ id: 'trello', group: 'sync', title: 'Sync with Trello', keywords: 'trello cards board tasks stories api key token person', component: TrelloSettings },
	{ id: 'screenshots', group: 'tools', title: 'Screenshots', keywords: 'screenshot hotkey shortcut flameshot capture picture tool command print', component: ScreenshotSettings },
	{ id: 'claude', group: 'tools', title: 'Claude Code', keywords: 'claude ai prompt run model permission guard rails cli program auto edit plan read', component: ClaudeCodeSettings },
	{ id: 'about', group: 'about', title: 'About Ledger', keywords: 'about version info project source code releases changelog machine paths config build', component: AboutSettings },
	{ id: 'updates', group: 'about', title: 'Updates', keywords: 'update updates upgrade new version release install check automatic', component: UpdateSettings },
	{ id: 'yell', group: 'logs', title: 'Yell at the app', keywords: 'yell report bug issue error github ai prompt copy problem broken', component: YellSettings },
	{ id: 'capture', group: 'logs', title: 'Debug capture', keywords: 'debug capture record recording start stop steps developer send file', component: CaptureSettings },
	{ id: 'log', group: 'logs', title: 'Log', keywords: 'logs debug errors problems clear folder size limit megabytes', component: LogSettings }
];

/** The panels every word of `query` appears in (in the title, the keywords or the group's name). */
export function searchPanels(query: string): Panel[] {
	const words = query.toLowerCase().split(/\s+/).filter(Boolean);
	if (!words.length) return [];
	return panels.filter((p) => {
		const group = groups.find((g) => g.id === p.group)?.label ?? '';
		const text = `${p.title} ${p.keywords} ${group}`.toLowerCase();
		return words.every((w) => text.includes(w));
	});
}
