import fs from 'fs/promises';
import {join as pathJoin} from 'path';
import {fileURLToPath} from 'url';

const CYCLES_PER_MS = 3600000;

const dataDir = process.env.DATA_DIR;
const templatePath = pathJoin(fileURLToPath(import.meta.url), '../template.out');
const template = await fs.readFile(templatePath, 'utf8');

const resultsPath = pathJoin(dataDir, 'results.json');
const results = JSON.parse(await fs.readFile(resultsPath, 'utf8'));
await fs.rm(resultsPath);

let pid = 4000;
for (const {name, ms} of results) {
    const componentName = name.replace(/\[.*$/, ''),
        cycles = Math.round(ms * CYCLES_PER_MS);
    const content = template
        .replace('<pid>', pid)
        .replace('<name>', name)
        .replace('<cycles>', cycles)
        .replace('<cycles-minus-one>', cycles - 1);
    await fs.writeFile(pathJoin(dataDir, `${componentName}_${pid}.out`), content);
    pid++;
}
