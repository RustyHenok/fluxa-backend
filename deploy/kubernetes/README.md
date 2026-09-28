# Reference Kubernetes manifests

Minimal, production-shaped manifests for fluxa-backend. Adjust namespaces,
image references, storage classes, and resource sizing for your cluster.

Apply order:

```bash
kubectl apply -f secret.example.yaml   # after filling in real values
kubectl apply -f artifacts-pvc.yaml
kubectl apply -f api.yaml
kubectl apply -f worker.yaml
```

Notes:

- `secret.example.yaml` is a template — copy it, fill in real values from your
  secret manager, and never commit populated secrets.
- The API Service exposes only HTTP; the gRPC port stays cluster-internal and
  is protected by `GRPC_AUTH_TOKEN`.
- Both Deployments mount the shared artifacts PVC (`ReadWriteMany`) so exports
  written by workers are downloadable through the API. If your cluster lacks a
  RWM storage class, run a single worker replica with `ReadWriteOnce` or move
  artifacts to object storage.
- See [`docs/operations.md`](../../docs/operations.md) for probes, metrics,
  scaling, and retention guidance.
