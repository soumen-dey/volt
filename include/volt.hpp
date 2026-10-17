#ifndef VOLT_HPP
#define VOLT_HPP

#include "volt.h"

#include <functional>
#include <memory>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace volt {

class Error : public std::runtime_error {
public:
    using std::runtime_error::runtime_error;
};

class Engine {
    struct CallbackState {
        std::function<double(const double *, size_t)> function;
    };

    struct Deleter {
        void operator()(VoltEngine *engine) const noexcept {
            volt_engine_free(engine);
        }
    };

    std::vector<std::unique_ptr<CallbackState>> callbacks_;
    std::unique_ptr<VoltEngine, Deleter> engine_;

    static int invoke_callback(
        void *user_data,
        const double *arguments,
        size_t argument_count,
        double *out_result
    ) noexcept {
        if (user_data == nullptr || out_result == nullptr) {
            return 1;
        }
        try {
            auto *state = static_cast<CallbackState *>(user_data);
            *out_result = state->function(arguments, argument_count);
            return 0;
        } catch (...) {
            return 1;
        }
    }

public:
    Engine() : engine_(volt_engine_new()) {
        if (!engine_) {
            throw Error("failed to create Volt engine");
        }
    }

    Engine(size_t max_source_bytes,
           size_t max_execution_steps,
           size_t max_call_depth,
           unsigned long long max_evaluation_time_ms)
        : engine_(volt_engine_new_with_budgets(
              max_source_bytes,
              max_execution_steps,
              max_call_depth,
              max_evaluation_time_ms
          )) {
        if (!engine_) {
            throw Error("failed to create Volt engine");
        }
    }

    Engine(const Engine &) = delete;
    Engine &operator=(const Engine &) = delete;
    Engine(Engine &&) = delete;
    Engine &operator=(Engine &&) = delete;

    std::string eval(const std::string &source) {
        if (source.find('\0') != std::string::npos) {
            throw Error("source contains an embedded NUL byte");
        }
        char *result = nullptr;
        const int status = volt_engine_eval(engine_.get(), source.c_str(), &result);
        if (status != VOLT_STATUS_OK) {
            const char *message = volt_engine_last_error(engine_.get());
            throw Error(message == nullptr ? "Volt evaluation failed" : message);
        }
        std::unique_ptr<char, decltype(&volt_string_free)> owned(result, volt_string_free);
        return std::string(owned.get());
    }

    void register_number_function(
        const std::string &name,
        std::function<double(const double *, size_t)> function
    ) {
        if (!function) {
            throw Error("callback must not be empty");
        }
        if (name.find('\0') != std::string::npos) {
            throw Error("host function name contains an embedded NUL byte");
        }
        auto state = std::make_unique<CallbackState>();
        state->function = std::move(function);
        callbacks_.push_back(std::move(state));
        const int status = volt_engine_register_number_fn(
            engine_.get(),
            name.c_str(),
            &Engine::invoke_callback,
            callbacks_.back().get()
        );
        if (status != VOLT_STATUS_OK) {
            const char *message = volt_engine_last_error(engine_.get());
            const std::string error =
                message == nullptr ? "Volt callback registration failed" : message;
            callbacks_.pop_back();
            throw Error(error);
        }
    }
};

} // namespace volt

#endif
