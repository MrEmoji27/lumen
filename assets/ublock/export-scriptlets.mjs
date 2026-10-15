// Turn uBlock Origin's scriptlets (src/js/resources/scriptlets.js) into adblock-rust resources.
// usage: node export-scriptlets.mjs <path/to/scriptlets.js> <out.json>
import { pathToFileURL } from 'node:url';
import { writeFileSync } from 'node:fs';
const [, , src, out] = process.argv;
const { builtinScriptlets } = await import(pathToFileURL(src).href);
const res = [];
for (const s of builtinScriptlets) {
    const code = s.fn.toString();
    // Dependencies (".fn") are pasted in as-is, classes included. A scriptlet itself must be a
    // named function: adblock-rust calls it by that name.
    if (!s.name.endsWith('.fn') && !/^function\s+[\w$]+\s*\(/.test(code)) { console.error('skip:', s.name); continue; }
    const r = {
        name: s.name,
        aliases: s.aliases || [],
        kind: { mime: s.name.endsWith('.fn') ? 'fn/javascript' : 'application/javascript' },
        content: Buffer.from(code).toString('base64'),
    };
    if (s.dependencies && s.dependencies.length) r.dependencies = s.dependencies;
    if (s.requiresTrust) r.permission = 1;
    res.push(r);
}
writeFileSync(out, JSON.stringify(res));
console.log(res.length, 'resources,', res.filter(r => r.permission).length, 'trusted');
