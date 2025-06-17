use goose::{config::ExtensionConfig, recipe::{Recipe, SubRecipe}};
use serde_json::json;

pub fn create_sub_recipe_extensions(recipe: &Recipe) -> Vec<ExtensionConfig> {
    let mut extensions: Vec<ExtensionConfig> = Vec::new();
    if let Some(sub_recipes) = &recipe.sub_recipes {
        for sub_recipe in sub_recipes {
            let sub_recipe_attributes = SubRecipe {
                path: sub_recipe.path.clone(),
                params: sub_recipe.params.clone(),
                name: sub_recipe.name.clone(),
            };
            println!("======= Sub recipe attributes: {:?}", &sub_recipe_attributes);
            let args = serde_json::to_string(&sub_recipe_attributes).unwrap_or_default();

            let extension = ExtensionConfig::Builtin {
                name: format!("sub-recipe-{}", sub_recipe.name),
                timeout: Some(300),
                bundled: Some(true),
                display_name: Some(format!("sub-recipe-{}", sub_recipe.name)),
                args: Some(vec![args]),
            };
            extensions.push(extension);
        }
    }
    extensions
}

pub fn create_sub_recipe_instructions(recipe: &Recipe) -> String {
    let mut instructions = String::new();
    if let Some(sub_recipes) = &recipe.sub_recipes {
        for sub_recipe in sub_recipes {
            instructions.push_str(&format!(
                "if {} is required to run, then use sub_recipe_run_{} tool in the sub-recipe-{} extension directly to run the sub-recipe. The tool knows how to run it \n", 
                sub_recipe.name, sub_recipe.name, sub_recipe.name));
        }
    }
    instructions
}

fn create_sub_recipe_command(sub_recipe: &SubRecipe) -> String {
    let mut command = String::new();
    command.push_str(&format!("goose run --recipe {} ", sub_recipe.path));
    if let Some(params) = &sub_recipe.params {
        for param in params {
            command.push_str(&format!(" --params {}={}", param.name, param.value));
        }
    }
    return command;
}