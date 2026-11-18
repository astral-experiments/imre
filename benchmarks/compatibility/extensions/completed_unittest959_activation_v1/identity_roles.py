"""Normalize direct supervised finite captures into the shared checked finite core."""

import hashlib
from functools import lru_cache
from pathlib import Path

import raw_control

NATIVE_TIERS = ("O0", "O3")
GATE_MODES = ("direct", "imported")
PRODUCT_ROLES = ("cli", "runtime", "clang")
TIMEOUT_POLICY = "absolute-monotonic-from-before-popen"


class DirectCapture:
    def __init__(self, api, binding, data, obj):
        self.api = api
        self.binding = binding
        self.data = data
        self.raw = obj

    def held(self, record):
        self.api.require(
            record["fstat_before_read"] == record["fstat_after_read"] == record["path_stat"],
            "gate held FD changed during read or differs from path",
        )
        return {
            "fd_realpath": record["canonical_path"],
            "path": record["path"],
            "fstat": record["fstat_before_read"],
            "sha256": record["sha256"],
        }

    def _stock_metadata(self, capture):
        self.api.require(
            capture["same_ordered_originals_and_outcomes"]
            and capture["stdout_identical"]
            and capture["stderr_identical"]
            and self.data("stock_stdout") == self.data("prior_stock_stdout")
            and self.data("stock_stderr") == self.data("prior_stock_stderr"),
            "stock capture differs from frozen observations",
        )
        self.api.require(
            self.data("stock_effects_before") == self.data("stock_effects_after"),
            "stock file effects changed",
        )
        self._check_timeout(capture, "stock")
        return {
            **capture,
            "original_stock_metadata_sha256": capture["original_metadata_sha256"],
            "ordered_streams_and_effects_equal": True,
        }

    def _check_timeout(self, capture, tier):
        self.api.require(
            capture["timeout_policy"] == TIMEOUT_POLICY and capture["timeout_seconds"] == 240,
            f"{tier} absolute timeout differs",
        )

    def _native_metadata(self, tier, capture):
        image = capture["argv"][-1]
        lines = self.data(f"{tier}_trace").decode().splitlines()
        _, child = raw_control.execs(lines, self.api.require)
        child_environment = raw_control.exec_environment(lines[child[0]], self.api.require)
        before, after = capture["held_fd_pre"], capture["held_fd_post"]
        sample = capture["proc_exe_sample"]
        self.api.require(
            before == after and before["fstat"] == capture["path_post_fstat"] == sample["fstat"],
            "native held executable identity changed",
        )
        self.api.require(
            sample["pid"] == child[1] and sample["target"] == image == child[2],
            "native live executable sample differs from traced exec",
        )
        self.api.require(
            capture["emergency_restores"] == [] and capture["same_ordered_originals_and_outcomes"],
            "native restoration or frozen outcomes differ",
        )
        self._check_timeout(capture, tier)
        return {
            **capture,
            "argv": [image],
            "capture_argv": capture["argv"],
            "environment": child_environment,
            "supervisor_environment": capture["environment"],
            "before": {image: before["sha256"]},
            "after": {image: after["sha256"]},
            "image_sha256": before["sha256"],
            "supervisor_sha256": self.binding["probe"]["executable_sha256"],
            "restored": True,
            "image_identity_before_after": True,
        }

    def _gate_metadata(self, tier, mode, capture):
        positive = self.raw(f"{tier}_build_json")
        lines = self.data(f"gate_{tier}_{mode}_trace").decode().splitlines()
        _, child = raw_control.execs(lines, self.api.require)
        sample = capture["live_compiler_sample"]
        held_cli = capture["held_before"]["cli"]
        self.api.require(
            capture["held_before"] == capture["held_after"], "gate held products changed"
        )
        self.api.require(
            capture["compiler_child_exit_code"]
            == raw_control.child_exit_code(lines, child[1], self.api.require)
            == 1,
            "gate compiler exit differs from raw trace",
        )
        self.api.require(
            sample["pid"] == int(child[1])
            and sample["target"] == child[2] == held_cli["canonical_path"]
            and sample["fstat"] == held_cli["fstat_before_read"]
            and capture["start_utc_ns"] < sample["sample_utc_ns"] < capture["finish_utc_ns"],
            "live gate compiler identity or sample time differs",
        )
        self.api.require(
            capture["negative_launcher_sha256"] == raw_control.sha(self.data(f"{mode}_launcher")),
            "gate negative launcher source differs",
        )
        poison_path = None
        if mode == "imported":
            poison_path = str(Path(child[3][2]).parent / f"{self.binding['poison_module']}.py")
        return {
            **capture,
            "original_launcher_sha256": raw_control.sha(self.data("positive_launcher")),
            "positive_build_metadata_sha256": raw_control.sha(self.data(f"{tier}_build_json")),
            "positive_build_argv": positive["build_argv"],
            "build_argv": child[3],
            "probe_sha256": self.binding["probe"]["executable_sha256"],
            "probe_source_sha256": self.binding["probe"]["source_sha256"],
            "supervisor_exit_code": capture["exit_code"],
            "actual_compiler_child_environment": raw_control.exec_environment(
                lines[child[0]], self.api.require
            ),
            "poison_path": poison_path,
        }

    @lru_cache(maxsize=None)
    def object(self, role):
        capture = self.raw(role)
        if role == "stock_meta":
            return self._stock_metadata(capture)
        if role in {f"{tier}_meta" for tier in NATIVE_TIERS}:
            return self._native_metadata(role[:2], capture)
        if role in {f"{tier}_build_json" for tier in NATIVE_TIERS}:
            return {
                **capture,
                "argv": capture["build_argv"],
                "exit_code": capture["compiler_exit_code"],
                "image_sha256": capture["new_image_sha256"],
            }
        if role.startswith("gate_") and role.endswith("_meta"):
            _, tier, mode, _ = role.split("_")
            return self._gate_metadata(tier, mode, capture)
        return capture

    def _gate_bracket(self):
        bracket = self.raw("gate_bracket")
        expected_gates = [f"{tier}.{mode}" for tier in NATIVE_TIERS for mode in GATE_MODES]
        self.api.require(
            bracket["held_before"] == bracket["held_after"] and bracket["gates"] == expected_gates,
            "global gate bracket or census differs",
        )
        return {
            "fd_before": {key: self.held(value) for key, value in bracket["held_before"].items()},
            "fd_after": {key: self.held(value) for key, value in bracket["held_after"].items()},
            "fd_unchanged": True,
            "controls": [
                {
                    "tier": tier,
                    "kind": mode,
                    "record_sha256": raw_control.sha(self.data(f"gate_{tier}_{mode}_meta")),
                }
                for tier in NATIVE_TIERS
                for mode in GATE_MODES
            ],
            "controller_sha256": bracket["controller_sha256"],
        }

    def _gate_identity(self, role):
        capture = self.object(role)
        predecessor = self.raw(f"prior_{role}")
        self.api.require(
            self.api.comparable_build(predecessor["target_argv"][3:])
            == self.api.comparable_build(capture["build_argv"]),
            "gate predecessor graph differs",
        )
        self.api.require(
            predecessor["held_before"] == predecessor["held_after"]
            and all(
                predecessor["held_before"][key]["sha256"] == capture["held_before"][key]["sha256"]
                for key in PRODUCT_ROLES
            ),
            "gate predecessor products differ",
        )
        return {
            **capture,
            "actual_compiler_path": capture["build_argv"][0],
            "actual_compiler_argv": capture["build_argv"],
            "actual_compiler_environment": capture["actual_compiler_child_environment"],
            "source_negative_sha256": capture["negative_launcher_sha256"],
            "fd_fstat_before": {
                key: record["fstat_before_read"] for key, record in capture["held_before"].items()
            },
            "fd_fstat_after": {
                key: record["fstat_after_read"] for key, record in capture["held_after"].items()
            },
            "corrected_gate_record_sha256": raw_control.sha(self.data(f"prior_{role}")),
        }

    @lru_cache(maxsize=None)
    def identity_object(self, role):
        if role in {f"{tier}_build_json" for tier in NATIVE_TIERS}:
            return self.raw(role)
        if role == "gate_bracket":
            return self._gate_bracket()
        if role.startswith("gate_") and role.endswith("_meta"):
            return self._gate_identity(role)
        if role.startswith("prior_") and role.endswith("_meta"):
            capture = self.raw(role)
            return {**capture, "argv": [capture["target_argv"][-1]]}
        return self.raw(role)

    def _check_source_and_products(self):
        binding = self.binding
        source = self.data("stock_source")
        blob = hashlib.sha1(f"blob {len(source)}\0".encode() + source).hexdigest()
        assessment = self.raw("assessment")
        self.api.require(
            raw_control.sha(source) == binding["source"]["sha256"]
            and blob == binding["source"]["git_blob"]
            and assessment["source"] == binding["source"]
            and assessment["revision"] == binding["revision"] == self.api.COHORT["imre_revision"],
            "direct source, blob, assessment or revision differs",
        )
        self.api.require(
            {key: assessment["products"][key] for key in PRODUCT_ROLES} == binding["product"]
            and assessment["products"]["stock"] == binding["stock_interpreter"]["sha256"],
            "direct compiler, runtime, clang or stock identity differs",
        )
        self.api.require(
            raw_control.sha(self.data("gate_probe_source")) == binding["probe"]["source_sha256"]
            and raw_control.sha(self.data("gate_probe_binary"))
            == binding["probe"]["executable_sha256"],
            "direct probe source or binary differs",
        )

    def _check_custody(self):
        custody = self.raw("post_custody")
        before = self.raw("capture_before")
        self.api.require(
            custody["controller_sha256"] == raw_control.sha(self.data("stock_controller"))
            and custody["baseline_sha256"] == raw_control.sha(self.data("capture_before")),
            "post-invocation custody controller or baseline differs",
        )
        self.api.require(
            custody["after"] == before
            and before["fixture_equal"] is True
            and before["fixture_files"] == len(self.raw("fixture_manifest")),
            "post-invocation fixture, source, product or image custody differs",
        )

    def context(self, entry=None):
        self._check_source_and_products()
        self._check_custody()
        binding = self.binding
        if entry is not None:
            authority = self.api.extension_authority(entry)
            stock = binding["stock_interpreter"]
            self.api.require(
                (stock["sha256"], stock["version"]) == authority.predecessor_stock(self.api),
                "direct stock differs from predecessor",
            )
            authority.donor(self.api, binding["product"])
        run = {
            "family": binding["family"],
            "revision": binding["revision"],
            "runs": {tier: self.object(f"{tier}_meta") for tier in self.api.TIERS},
        }
        return {
            "legacy": False,
            "run": run,
            "platform": dict(self.api.COHORT["platform"]),
            "gate_tiers": NATIVE_TIERS,
            "poison_module": binding["poison_module"],
            "operations": binding["operations"],
            "probe": binding["probe"],
            "observed": len(binding["originals"]),
            "withdrawal": binding["withdrawal"],
            "stock_interpreter": binding["stock_interpreter"],
            "identity": {"held_proof": "read-bracket-v1"},
            "roles": binding["roles"],
            "join_modes": binding["join_modes"],
            "raw_obj": self.raw,
            "identity_obj": self.identity_object,
            "product": binding["product"],
            "source": binding["source"],
            "completion": {"schema": "imre.compatibility.completed-unittest.v1"},
            "ordered_originals": [(row["id"], row["occurrence"]) for row in binding["originals"]],
            "tier_environment": {
                "schema": "imre.compatibility.finite-tier-environment.v1",
                "tiers": {tier: "stock-unchanged" for tier in NATIVE_TIERS},
            },
            "build_controller_roles": {tier: f"{tier}_build_controller" for tier in NATIVE_TIERS},
        }
