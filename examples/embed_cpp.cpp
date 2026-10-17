#include "volt.hpp"

#include <stdexcept>

int main() {
    volt::Engine engine;
    engine.register_number_function(
        "doubleValue",
        [](const double *arguments, size_t count) {
            if (count != 1) {
                throw std::runtime_error("doubleValue expects one argument");
            }
            return arguments[0] * 2.0;
        }
    );

    if (engine.eval("let answer = doubleValue(21); answer") != "42") {
        return 1;
    }
    if (engine.eval("answer + 1") != "43") {
        return 1;
    }

    try {
        engine.eval(std::string("1\0 + 2", 6));
        return 1;
    } catch (const volt::Error &) {
    }

    try {
        engine.eval("missingBinding");
    } catch (const volt::Error &) {
        return 0;
    }
    return 1;
}
