# Website deployment

The Next.js website lives in `website/` and is deployed to Vercel through
GitHub Actions. Vercel's automatic Git deployments are disabled in
`website/vercel.json` so a commit creates only one deployment.

## Pipeline

| Workflow | Trigger | Result |
| --- | --- | --- |
| Website CI | Pull requests to `main` that change `website/` | Frozen install, lint, and production build |
| Vercel Preview | Pushes to non-`main` branches that change `website/` | Unique Vercel Preview deployment |
| Vercel Production | Pushes to `main` that change `website/` | Production deployment and domain promotion |

Preview deployment intentionally runs on branch pushes rather than the
`pull_request_target` event. Pull requests from forks can run CI without
receiving Vercel credentials.

## 1. Create the Vercel project

1. In Vercel, select **Add New → Project** and import `HakimIno/plusplus`.
2. Set **Root Directory** to `website`.
3. Keep the detected **Next.js** framework and Bun install/build defaults.
4. Set the Production Branch to `main`.
5. Create or finish importing the project.

The same link can be created from a terminal:

```bash
cd website
bunx vercel@latest login
bunx vercel@latest link
```

The link command writes `.vercel/project.json`. The directory is ignored by
Git and must not be committed.

## 2. Add GitHub Actions credentials

Create a Vercel access token, then add these repository secrets under
**GitHub → Settings → Secrets and variables → Actions**:

| Secret | Value |
| --- | --- |
| `VERCEL_TOKEN` | Vercel access token |
| `VERCEL_ORG_ID` | `orgId` from `.vercel/project.json` |
| `VERCEL_PROJECT_ID` | `projectId` from `.vercel/project.json` |

The project and organization IDs are also available in the Vercel project
settings.

## 3. Create deployment environments

Under **GitHub → Settings → Environments**, create:

- `preview`
- `production`

For `production`, add required reviewers or a deployment branch rule restricted
to `main` if the repository plan supports them. The workflow itself also refuses
to deploy production from any branch other than `main`.

The Vercel secrets can remain repository secrets. If stricter separation is
needed, move copies into both GitHub environments instead.

## 4. Run the first deployment

Commit and push these files to `main`. The **Vercel Production** workflow will
build and deploy the website. Its GitHub job summary and environment page show
the deployed URL.

To test Preview first:

```bash
git switch -c website/vercel-preview
git push -u origin website/vercel-preview
```

Any change under `website/` on that branch triggers **Vercel Preview**.

## Custom domain

Add `plusplus.dev` under **Vercel → Project → Settings → Domains**, then create
the DNS records Vercel provides. The website metadata already uses
`https://plusplus.dev` as its canonical base URL.

## Branch protection

For `main`, require the **Website CI / Lint and build** check before merging.
This keeps pull request validation separate from deployment while Production
still repeats lint before it publishes.
