/*
 * Read benchmark results file and run benchmark again, without any NAPI calls.
 * It's a workaround for CodSpeed's measurement of JS + NAPI being wildly inaccurate.
 * https://github.com/CodSpeedHQ/action/issues/96
 * So instead in CI we run the actual benchmark outside CodSpeed's instrumentation
 * (see `.github/workflows/benchmark.yml` and `napi/parser/parse.bench.mjs`).
 * `parse.bench.mjs` writes the measurements for the benchmarks to a file `results.json`.
 * This pseudo-benchmark reads that file and just busy-loops for the specified time.
 */

import fs from 'fs/promises';
import {join as pathJoin} from 'path';
import {Bench} from 'tinybench';
import {withCodSpeed} from "@codspeed/tinybench-plugin";

const resultsPath = pathJoin(process.env.DATA_DIR, 'results.json');
const files = JSON.parse(await fs.readFile(resultsPath));
await fs.rm(resultsPath);

const bench = withCodSpeed(new Bench());

for (const {filename, duration} of files) {
    bench.add(`parser_napi[${filename}]`, () => {
        const endTime = performance.now() + (duration * 1000);
        while (performance.now() < endTime) ;
    });
}

console.log('Running benchmarks');
await bench.run();
console.table(bench.table());
