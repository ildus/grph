/// Get the maximum number of explore calls based on project size
pub fn explore_budget(file_count: u32) -> u32 {
    match file_count {
        0..=499 => 1,
        500..=4999 => 2,
        5000..=14999 => 3,
        15000..=24999 => 4,
        _ => 5,
    }
}

/// Output budget for explore results
pub struct ExploreOutputBudget {
    pub max_chars: usize,
    pub max_files: usize,
    pub max_chars_per_file: usize,
}

pub fn explore_output_budget(file_count: u32) -> ExploreOutputBudget {
    match file_count {
        0..=499 => ExploreOutputBudget {
            max_chars: 18000,
            max_files: 8,
            max_chars_per_file: 3800,
        },
        500..=4999 => ExploreOutputBudget {
            max_chars: 28000,
            max_files: 10,
            max_chars_per_file: 6500,
        },
        5000..=14999 => ExploreOutputBudget {
            max_chars: 35000,
            max_files: 12,
            max_chars_per_file: 7000,
        },
        15000..=24999 => ExploreOutputBudget {
            max_chars: 38000,
            max_files: 14,
            max_chars_per_file: 7000,
        },
        _ => ExploreOutputBudget {
            max_chars: 38000,
            max_files: 14,
            max_chars_per_file: 7000,
        },
    }
}
