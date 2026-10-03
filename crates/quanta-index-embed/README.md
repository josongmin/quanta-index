# Embedding providers

Provider implementations and cache for semantic indexing/querying. The
searchd composition root chooses a profile and supplies egress authority;
this crate does not decide that policy for a caller.

Start with [provider exports](src/lib.rs), [Model2Vec](src/model2vec.rs),
[OpenAI transport](src/openai.rs), and [cache](src/cache.rs). Default/provider
policy is described in the [SEP-26-002 ADR](../../docs/adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md).
