from __future__ import annotations

import ctypes
import os
from pathlib import Path
import sys
from typing import Callable, Optional


class VoltError(RuntimeError):
    """Raised when Volt fails to create an engine or evaluate source."""


_NumberCallback = ctypes.CFUNCTYPE(
    ctypes.c_int,
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_double),
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_double),
)


def _library_names() -> tuple[str, ...]:
    if sys.platform == "win32":
        return ("volt_engine.dll",)
    if sys.platform == "darwin":
        return ("libvolt_engine.dylib",)
    return ("libvolt_engine.so",)


def _candidate_libraries() -> list[Path]:
    explicit = os.environ.get("VOLT_ENGINE_LIBRARY")
    candidates = [Path(explicit)] if explicit else []
    root = Path(__file__).resolve().parents[4]
    for name in _library_names():
        candidates.extend(
            [
                Path(__file__).resolve().parent / name,
                root / "target" / "release" / name,
                root / "target" / "debug" / name,
            ]
        )
    return candidates


def _load_library() -> ctypes.CDLL:
    failures = []
    for path in _candidate_libraries():
        if not path.is_file():
            continue
        try:
            return ctypes.CDLL(str(path))
        except OSError as error:
            failures.append(f"{path}: {error}")
    searched = ", ".join(str(path) for path in _candidate_libraries())
    detail = "; ".join(failures)
    raise VoltError(
        "Volt native library not found. Set VOLT_ENGINE_LIBRARY to the "
        f"library path; searched: {searched}"
        + (f". Load errors: {detail}" if detail else "")
    )


class _Api:
    def __init__(self) -> None:
        self.library = _load_library()
        self.library.volt_engine_new.restype = ctypes.c_void_p
        self.library.volt_engine_new_with_budgets.argtypes = [
            ctypes.c_size_t,
            ctypes.c_size_t,
            ctypes.c_size_t,
            ctypes.c_ulonglong,
        ]
        self.library.volt_engine_new_with_budgets.restype = ctypes.c_void_p
        self.library.volt_engine_free.argtypes = [ctypes.c_void_p]
        self.library.volt_engine_eval.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        self.library.volt_engine_eval.restype = ctypes.c_int
        self.library.volt_engine_last_error.argtypes = [ctypes.c_void_p]
        self.library.volt_engine_last_error.restype = ctypes.c_char_p
        self.library.volt_string_free.argtypes = [ctypes.c_void_p]
        self.library.volt_engine_register_number_fn.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            _NumberCallback,
            ctypes.c_void_p,
        ]
        self.library.volt_engine_register_number_fn.restype = ctypes.c_int


_api: Optional[_Api] = None


def _get_api() -> _Api:
    global _api
    if _api is None:
        _api = _Api()
    return _api


class Engine:
    """Owns one Volt engine. Instances are not safe for concurrent use."""

    def __init__(
        self,
        *,
        max_source_bytes: Optional[int] = None,
        max_execution_steps: Optional[int] = None,
        max_call_depth: Optional[int] = None,
        max_evaluation_time_ms: Optional[int] = None,
    ) -> None:
        self._api = _get_api()
        limits = (
            max_source_bytes,
            max_execution_steps,
            max_call_depth,
            max_evaluation_time_ms,
        )
        self._engine = ctypes.c_void_p()
        if all(value is None for value in limits):
            self._engine = ctypes.c_void_p(self._api.library.volt_engine_new())
        elif any(value is None for value in limits):
            raise ValueError("provide all four limits or omit all of them")
        else:
            for value in limits:
                if value is None or value < 0:
                    raise ValueError("limits must be non-negative integers")
            self._engine = ctypes.c_void_p(
                self._api.library.volt_engine_new_with_budgets(
                    max_source_bytes,
                    max_execution_steps,
                    max_call_depth,
                    max_evaluation_time_ms,
                )
            )
        if not self._engine:
            raise VoltError("failed to create Volt engine")
        self._callbacks: list[_NumberCallback] = []

    def eval(self, source: str) -> str:
        if not self._engine:
            raise VoltError("engine is closed")
        if "\0" in source:
            raise ValueError("source contains an embedded NUL byte")
        encoded = source.encode("utf-8")
        result = ctypes.c_void_p()
        status = self._api.library.volt_engine_eval(
            self._engine, encoded, ctypes.byref(result)
        )
        if status != 0:
            raise self._last_error()
        try:
            return ctypes.string_at(result).decode("utf-8")
        finally:
            self._api.library.volt_string_free(result)

    def register_number_function(
        self, name: str, callback: Callable[[list[float]], float]
    ) -> None:
        if not self._engine:
            raise VoltError("engine is closed")
        if not callable(callback):
            raise TypeError("callback must be callable")
        if "\0" in name:
            raise ValueError("host function name contains an embedded NUL byte")

        @_NumberCallback
        def trampoline(_user_data, arguments, count, output):
            try:
                values = [arguments[index] for index in range(count)]
                output[0] = float(callback(values))
                return 0
            except BaseException:
                return 1

        status = self._api.library.volt_engine_register_number_fn(
            self._engine, name.encode("utf-8"), trampoline, None
        )
        if status != 0:
            raise self._last_error()
        self._callbacks.append(trampoline)

    def _last_error(self) -> VoltError:
        message = self._api.library.volt_engine_last_error(self._engine)
        if message:
            return VoltError(message.decode("utf-8", errors="replace"))
        return VoltError("Volt operation failed")

    def close(self) -> None:
        if self._engine:
            self._api.library.volt_engine_free(self._engine)
            self._engine = ctypes.c_void_p()
            self._callbacks.clear()

    def __enter__(self) -> "Engine":
        if not self._engine:
            raise VoltError("engine is closed")
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        self.close()

    def __del__(self) -> None:
        if getattr(self, "_engine", None):
            self.close()


__all__ = ["Engine", "VoltError"]
