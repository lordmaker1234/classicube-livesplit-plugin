pub trait Module {
    fn free(&mut self) {}
    fn reset(&mut self) {}
    fn on_new_map(&mut self) {}
    fn on_new_map_loaded(&mut self) {}

    fn children(&mut self) -> Vec<&mut dyn Module> {
        vec![]
    }

    fn children_call_order(&mut self) -> Vec<&mut dyn Module> {
        let mut children = self.children();
        children.reverse();
        children
    }

    fn handle_free(&mut self) {
        for child in self.children_call_order() {
            child.handle_free();
        }
        self.free();
    }
    fn handle_reset(&mut self) {
        for child in self.children_call_order() {
            child.handle_reset();
        }
        self.reset();
    }
    fn handle_on_new_map(&mut self) {
        for child in self.children_call_order() {
            child.handle_on_new_map();
        }
        self.on_new_map();
    }
    fn handle_on_new_map_loaded(&mut self) {
        for child in self.children_call_order() {
            child.handle_on_new_map_loaded();
        }
        self.on_new_map_loaded();
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::Module;

    struct TestModule {
        name: &'static str,
        log: Rc<RefCell<Vec<&'static str>>>,
        children: Vec<TestModule>,
    }

    impl Module for TestModule {
        fn free(&mut self) {
            self.log.borrow_mut().push(self.name);
        }

        fn children(&mut self) -> Vec<&mut dyn Module> {
            self.children
                .iter_mut()
                .map(|c| c as &mut dyn Module)
                .collect()
        }
    }

    fn leaf(name: &'static str, log: &Rc<RefCell<Vec<&'static str>>>) -> TestModule {
        TestModule {
            name,
            log: log.clone(),
            children: vec![],
        }
    }

    #[test]
    fn handle_free_calls_children_in_reverse_then_self() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut root = TestModule {
            name: "root",
            log: log.clone(),
            children: vec![leaf("a", &log), leaf("b", &log)],
        };

        root.handle_free();

        assert_eq!(*log.borrow(), vec!["b", "a", "root"]);
    }

    #[test]
    fn handle_free_recurses_into_grandchildren() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut root = TestModule {
            name: "root",
            log: log.clone(),
            children: vec![
                TestModule {
                    name: "a",
                    log: log.clone(),
                    children: vec![leaf("a1", &log), leaf("a2", &log)],
                },
                leaf("b", &log),
            ],
        };

        root.handle_free();

        assert_eq!(*log.borrow(), vec!["b", "a2", "a1", "a", "root"]);
    }
}
