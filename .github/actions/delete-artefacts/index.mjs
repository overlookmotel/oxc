import * as core from '@actions/core';
import artifact from '@actions/artifact';

const artifactClient = artifact.default;

const REPO_OWNER = process.env.GITHUB_REPOSITORY_OWNER,
    REPO_NAME = process.env.GITHUB_REPOSITORY.slice(REPO_OWNER.length + 1);

try {
    await run();
} catch (err) {
    core.setFailed(err?.stack || 'Unknown error');
    console.log('FAILED');
}

async function run() {
    const workflowRunId = core.getInput('workflow_run_id') || null,
        token = core.getInput('token');

    const artefacts = await getArtefactsForWorkflow(workflowRunId, token);
    await Promise.all(artefacts.map(artefact => deleteArtefact(artefact, workflowRunId, token)));

    console.log('Done');
}

async function getArtefactsForWorkflow(workflowRunId, token) {
    const res = await artifactClient.listArtifacts(getOptions(workflowRunId, token));
    return res.artifacts;
}

async function deleteArtefact(artefact, workflowRunId, token) {
    await artifactClient.deleteArtifact(artefact.name, getOptions(workflowRunId, token));
}

function getOptions(workflowRunId, token) {
    if (!workflowRunId) return;
    return {
        findBy: {
            token,
            workflowRunId,
            repositoryOwner: REPO_OWNER,
            repositoryName: REPO_NAME,
        }
    };
}
