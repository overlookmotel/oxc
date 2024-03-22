import {join as pathJoin} from 'path';
import {writeFile} from 'fs/promises';
import {Bench} from 'tinybench';
import {parseSyncRaw} from './index.js';
import deserialize from './deserialize.js';
import fixtures from './fixtures.mjs';

const IS_CI = !!process.env.CI,
    ACCURATE = IS_CI || !!process.env.ACCURATE,
    CODSPEED = !!process.env.CODSPEED,
    DESERIALIZE_ONLY = !!process.env.DESERIALIZE_ONLY;

let bench = new Bench(
    ACCURATE
    ? {
        warmupIterations: 20, // Default is 5
        time: 5000, // 5 seconds, default is 500 ms
        iterations: 100, // Default is 10
    }
    : undefined
);
if (CODSPEED) {
    const {withCodSpeed} = await import('@codspeed/tinybench-plugin');
    bench = withCodSpeed(bench);
}

for (const {filename, sourceBuff, allocSize} of fixtures) {
    function getBuffer() {
        return parseSyncRaw(sourceBuff, {sourceFilename: filename}, allocSize);
    }
    function deser(buff) {
        deserialize(buff, sourceBuff);
    }

    if (DESERIALIZE_ONLY) {
        let buff;
        bench.add(
            `parser_napi_deser[${filename}]`,
            () => { deser(buff); },
            {beforeAll() { buff = getBuffer(); }}
        )
    } else {
        bench.add(`parser_napi${CODSPEED ? '_inst' : ''}[${filename}]`, () => {
            deser(getBuffer());
        });
    }
}

console.log('Warming up');
await bench.warmup();
console.log('Running benchmarks');
await bench.run();
console.table(bench.table());

// If running on CI, save results to file
if (IS_CI && !CODSPEED) {
    const dataDir = process.env.DATA_DIR;
    const results = bench.tasks.map(task => ({
        filename: task.name.match(/\[(.+)\]$/)[1],
        duration: task.result.period / 1000, // In seconds
    }));
    await writeFile(pathJoin(dataDir, 'results.json'), JSON.stringify(results));
}

// Dummy comment to run benches
