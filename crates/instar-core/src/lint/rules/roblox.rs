use super::super::syntax::{Context, array, number};

use serde_json::Value;

pub(super) fn apply(context: &mut Context<'_>, value: &Value, path: &str) {
    if !context.roblox {
        return;
    }

    let arguments = array(&value["args"]);

    if path == "Color3.new"
        && arguments
            .iter()
            .filter_map(number)
            .any(|channel| channel > 1.0)
    {
        context.emit(
            "color_bounds",
            value,
            "Color3.new channels use the unit scale",
        );
    }

    if path != "UDim2.new" {
        return;
    }

    if arguments.len() == 2 {
        context.emit(
            "dimension_arguments",
            value,
            "UDim2.new takes four arguments",
        );
    }

    if arguments.len() == 4 {
        if number(&arguments[1]) == Some(0.0) && number(&arguments[3]) == Some(0.0) {
            context.fix(
                "dimension_constructor",
                value,
                format!(
                    "UDim2.fromScale({}, {})",
                    context.text(&arguments[0]),
                    context.text(&arguments[2])
                ),
            );
        } else if number(&arguments[0]) == Some(0.0) && number(&arguments[2]) == Some(0.0) {
            context.fix(
                "dimension_constructor",
                value,
                format!(
                    "UDim2.fromOffset({}, {})",
                    context.text(&arguments[1]),
                    context.text(&arguments[3])
                ),
            );
        }
    }
}
