use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{CustomFilter, FilterChange};

use crate::file_object::FileObject;

pub struct SearchState {
    pub filter: CustomFilter,
    query: Rc<RefCell<String>>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchState {
    pub fn new() -> Self {
        let query = Rc::new(RefCell::new(String::new()));
        let query_ref = query.clone();

        let filter = CustomFilter::new(move |obj| {
            let q = query_ref.borrow();
            if q.is_empty() {
                return true;
            }
            let file = obj.downcast_ref::<FileObject>().unwrap();
            file.search_string().contains(q.as_str())
        });

        Self { filter, query }
    }

    pub fn set_query(&self, text: &str) {
        let lower = text.to_lowercase();
        *self.query.borrow_mut() = lower;
        self.filter.changed(FilterChange::Different);
    }

    pub fn is_active(&self) -> bool {
        !self.query.borrow().is_empty()
    }

    pub fn clear(&self) {
        *self.query.borrow_mut() = String::new();
        self.filter.changed(FilterChange::Different);
    }
}
