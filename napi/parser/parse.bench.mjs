import { fileURLToPath } from 'url';
import {dirname, join as pathJoin} from 'path';
import {readFile, writeFile} from 'fs/promises';
import {bench} from 'vitest';
import {parseSync} from './index.js';

const urls = [
    // TypeScript syntax (2.81MB)
    'https://raw.githubusercontent.com/microsoft/TypeScript/v5.3.3/src/compiler/checker.ts',
    // Real world app tsx (1.0M)
    'https://raw.githubusercontent.com/oxc-project/benchmark-files/main/cal.com.tsx',
    // Real world content-heavy app jsx (3K)
    'https://raw.githubusercontent.com/oxc-project/benchmark-files/main/RadixUIAdoptionSection.jsx',
    // Heavy with classes (554K)
    'https://cdn.jsdelivr.net/npm/pdfjs-dist@4.0.269/build/pdf.mjs',
    // ES5 (3.9M)
    'https://cdn.jsdelivr.net/npm/antd@5.12.5/dist/antd.js',
];

const cacheDirPath = pathJoin(dirname(fileURLToPath(import.meta.url)), '../../target');

const files = await Promise.all(urls.map(async (url) => {
    const filename = url.split('/').at(-1),
        path = pathJoin(cacheDirPath, filename);

    let code;
    try {
        code = await readFile(path, 'utf8');
        console.log('Got from FS cache:', filename);
    } catch {
        console.log('Downloading:', filename);
        const res = await fetch(url);
        code = await res.text();
        await writeFile(path, code);
    }

    return {filename, code};
}));

for (const {filename, code} of files) {
    bench(`parser(napi)[${filename}]`, () => {
        parseSync(code, {sourceFilename: filename});
    });
}
