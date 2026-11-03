// The AI-use labels of the Me & My Machine (MMM) scheme, https://mmmlabel.tech — made by Fontys University of Applied
// Sciences with welgeen and technofilosofie.com. A label is self-declared: it says how much of a product generative AI made,
// from almost all of it to none. Ledger records who made each change; the level is the person's to declare.
// Kept free of Svelte and Tauri imports so it can be exercised on its own.

import type { MmmLevel, ProvenanceVersion } from './types';

export interface Mmm {
	level: MmmLevel;
	name: string;
	/** The site's one-line name for the level. */
	tagline: string;
	/** What the level means, from the site. */
	meaning: string;
	/** A colour for the dot; the text stays in the theme's ink so every theme keeps its contrast. */
	color: string;
}

/** From the most AI to the least. */
export const MMM: readonly Mmm[] = [
	{
		level: 'lazy-prompter',
		name: 'Lazy Prompter',
		tagline: 'Made by GenAI',
		meaning: 'GenAI produced almost everything, with some minimal prompting and perhaps a little modification.',
		color: '#e26d6d'
	},
	{
		level: 'piggybacker',
		name: 'Piggybacker',
		tagline: 'Mostly made by GenAI',
		meaning: 'GenAI did most of the work; the human did basic prompting, reprompting and curating of results.',
		color: '#e0a645'
	},
	{
		level: 'cyborg',
		name: 'Cyborg',
		tagline: 'Made together with GenAI',
		meaning: 'Human and machine collaborated closely: elaborate prompting, human review, rewriting or reprompting.',
		color: '#7c7cf0'
	},
	{
		level: 'handyman',
		name: 'Handyman',
		tagline: 'Assisted by GenAI',
		meaning: 'GenAI played a supporting role (inspiration, correction), or was used heavily but the human still had to work harder.',
		color: '#4ea8ff'
	},
	{
		level: 'man-made',
		name: 'Man-Made',
		tagline: 'Made without GenAI',
		meaning: 'The product contains no generative AI.',
		color: '#3ec28f'
	}
];

export const mmmInfo = (level: string | null | undefined): Mmm | undefined => MMM.find((m) => m.level === level);

/** Who made a version, in words: Claude Code when Ledger saw a run do it, "AI" when the person said so, "You" when they typed it. */
export function authorLabel(v: Pick<ProvenanceVersion, 'by' | 'run'>): string {
	if (v.by === 'ai') return v.run ? 'Claude Code' : 'AI, as declared';
	if (v.by === 'human') return 'You';
	return 'Origin not recorded';
}

/** "+312 B", "−1.2 KB": how a version's size moved against the one before it. */
export function sizeChange(bytes: number): string {
	const abs = Math.abs(bytes);
	const sign = bytes < 0 ? '−' : '+';
	if (abs < 1024) return `${sign}${abs} B`;
	return `${sign}${(abs / 1024).toFixed(abs < 10 * 1024 ? 1 : 0)} KB`;
}
