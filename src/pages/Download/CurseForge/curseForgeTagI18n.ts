import type { TFunction } from 'i18next';

const normalizeKey = (value: string) => {
    const base = value
        .trim()
        .toLowerCase()
        .replace(/\+/g, ' plus ')
        .replace(/&/g, ' and ')
        .replace(/[’']/g, '')
        .replace(/[,()]/g, ' ')
        .replace(/[^a-z0-9]+/g, '_')
        .replace(/^_+|_+$/g, '')
        .replace(/_+/g, '_');

    if (!base) return '';
    if (/^\d/.test(base)) return `tag_${base}`;
    return base;
};

export const tCurseForgeTag = (
    t: TFunction,
    input:
        | string
        | { name?: string; slug?: string }
        | null
        | undefined,
) => {
    const name = typeof input === 'string' ? input : (input?.name || '');
    if (!name) return '';

    const slug = typeof input === 'string' ? undefined : input?.slug;
    const keyPart = slug ? normalizeKey(slug) : normalizeKey(name);
    if (!keyPart) return name;

    return t(`CurseForge.tags.${keyPart}`, { defaultValue: name });
};

