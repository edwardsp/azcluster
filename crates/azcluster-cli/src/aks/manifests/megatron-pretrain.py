"""Megatron-Bridge Llama-3.1-8B pretraining launcher (mock data) for azcluster AKS.

Replicates the Megatron-Bridge `llama31_8b_pretrain_config` recipe but sources the
model architecture from the non-gated `NousResearch/Meta-Llama-3.1-8B` mirror so no
HuggingFace gated-repo token is needed, and uses the NullTokenizer + mock dataset
(the recipe default) so there is no dataset to stage — the run is a pure
strong-scaling throughput benchmark reporting MODEL_TFLOP/s/GPU.

Launched by torchrun (PyTorchJob); parallelism/batch/iters come from env vars.
"""

import os

from megatron.bridge.models import AutoBridge
from megatron.bridge.recipes.llama.llama3 import (
    _pretrain_common,
    DEFAULT_NULL_TOKENIZER_VOCAB_SIZE,
)
from megatron.bridge.training.gpt_step import forward_step
from megatron.bridge.training.pretrain import pretrain

cfg = _pretrain_common()
cfg.model = AutoBridge.from_hf_pretrained(
    os.environ.get("HF_MODEL", "NousResearch/Meta-Llama-3.1-8B")
).to_megatron_provider(load_weights=False)
cfg.tokenizer.tokenizer_type = "NullTokenizer"
cfg.tokenizer.tokenizer_model = None
cfg.tokenizer.vocab_size = DEFAULT_NULL_TOKENIZER_VOCAB_SIZE
cfg.dataset.blend = None
cfg.dataset.seq_length = 8192
cfg.model.tensor_model_parallel_size = int(os.environ.get("TP", "1"))
cfg.model.pipeline_model_parallel_size = int(os.environ.get("PP", "1"))
cfg.model.context_parallel_size = int(os.environ.get("CP", "2"))
cfg.model.seq_length = 8192
cfg.model.transformer_impl = "transformer_engine"
cfg.train.train_iters = int(os.environ.get("TRAIN_ITERS", "50"))
cfg.train.global_batch_size = int(os.environ.get("GBS", "256"))
cfg.train.micro_batch_size = int(os.environ.get("MBS", "1"))
cfg.logger.log_interval = 1

pretrain(config=cfg, forward_step_func=forward_step)
