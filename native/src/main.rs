use herdr_queue::entry::{execute, parse_args};
use herdr_queue::models::Error;
use std::collections::HashMap;

fn main() {
    let arguments = std::env::args_os()
        .skip(1)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| Error::new("Les arguments doivent être du texte UTF-8."))
        })
        .collect::<Result<Vec<_>, _>>();
    let environment: HashMap<_, _> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let result = arguments
        .and_then(|arguments| parse_args(&arguments))
        .and_then(|command| execute(command, &environment));
    if let Err(error) = result {
        eprintln!("Herdr Queue : {error}");
        std::process::exit(1);
    }
}
