//! A small entity-component-system.
//!
//! - An [`Entity`] is an index plus a generation. Despawning bumps the generation, so a stale handle never matches a
//!   reused slot.
//! - A component is any `'static` type. Each component type has its own store, keyed by entity. Stores iterate in
//!   entity order, so systems behave the same on every run.
//! - A resource is a single value shared by all systems (for example the time step).
//! - A [`Schedule`] runs systems in the order they were added.
//!
//! Scene code uses this to keep objects distinct (each entity has its own components) and programmable (systems read
//! and write components, and behaviours can be attached or detached at run time).

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::str::FromStr;

/// A handle to an entity. Two entities are equal only if both index and generation match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Written as `index.generation`, for example `3.0`. This is the id the console prints and accepts.
impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.index, self.generation)
    }
}

impl FromStr for Entity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (index, generation) = s.split_once('.').ok_or_else(|| format!("entity id `{s}` must look like 3.0"))?;
        Ok(Entity {
            index: index.parse().map_err(|_| format!("bad entity index in `{s}`"))?,
            generation: generation.parse().map_err(|_| format!("bad entity generation in `{s}`"))?,
        })
    }
}

/// The store for one component type, behind a type-erased interface.
trait AnyStore: Any {
    fn remove(&mut self, entity: Entity) -> bool;
    fn has(&self, entity: Entity) -> bool;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct Store<T: 'static> {
    values: BTreeMap<Entity, T>,
}

impl<T: 'static> AnyStore for Store<T> {
    fn remove(&mut self, entity: Entity) -> bool {
        self.values.remove(&entity).is_some()
    }
    fn has(&self, entity: Entity) -> bool {
        self.values.contains_key(&entity)
    }
    fn name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Default)]
pub struct World {
    generations: Vec<u32>,
    alive: Vec<bool>,
    free: Vec<u32>,
    stores: HashMap<TypeId, Box<dyn AnyStore>>,
    resources: HashMap<TypeId, Box<dyn Any>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an entity with no components. Reuses a freed slot when there is one.
    pub fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            self.alive[index as usize] = true;
            return Entity { index, generation: self.generations[index as usize] };
        }
        let index = self.generations.len() as u32;
        self.generations.push(0);
        self.alive.push(true);
        Entity { index, generation: 0 }
    }

    /// Removes an entity and all its components. Returns `false` if the handle was already stale.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        for store in self.stores.values_mut() {
            store.remove(entity);
        }
        let slot = entity.index as usize;
        self.alive[slot] = false;
        self.generations[slot] += 1;
        self.free.push(entity.index);
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index as usize;
        slot < self.alive.len() && self.alive[slot] && self.generations[slot] == entity.generation
    }

    /// Every living entity, in index order.
    pub fn entities(&self) -> Vec<Entity> {
        (0..self.alive.len())
            .filter(|&i| self.alive[i])
            .map(|i| Entity { index: i as u32, generation: self.generations[i] })
            .collect()
    }

    /// Adds or replaces a component. Fails for a stale or unknown entity.
    pub fn insert<T: 'static>(&mut self, entity: Entity, component: T) -> Result<(), String> {
        if !self.is_alive(entity) {
            return Err(format!("entity {entity} is not alive"));
        }
        let store = self.stores.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(Store::<T> { values: BTreeMap::new() }));
        let typed = store.as_any_mut().downcast_mut::<Store<T>>().expect("store matches its type id");
        typed.values.insert(entity, component);
        Ok(())
    }

    pub fn get<T: 'static>(&self, entity: Entity) -> Option<&T> {
        self.typed::<T>()?.values.get(&entity)
    }

    pub fn get_mut<T: 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        self.typed_mut::<T>()?.values.get_mut(&entity)
    }

    pub fn has<T: 'static>(&self, entity: Entity) -> bool {
        self.typed::<T>().is_some_and(|s| s.values.contains_key(&entity))
    }

    pub fn remove<T: 'static>(&mut self, entity: Entity) -> Option<T> {
        self.typed_mut::<T>()?.values.remove(&entity)
    }

    /// Every entity that has a `T`, in entity order, with a reference to it.
    pub fn query<T: 'static>(&self) -> Vec<(Entity, &T)> {
        self.typed::<T>().map_or_else(Vec::new, |s| s.values.iter().map(|(e, v)| (*e, v)).collect())
    }

    /// The entities that have a `T`, in entity order. Use with `get_mut` to update each one.
    pub fn with<T: 'static>(&self) -> Vec<Entity> {
        self.typed::<T>().map_or_else(Vec::new, |s| s.values.keys().copied().collect())
    }

    /// The names of the component types an entity has, for inspection.
    pub fn component_names(&self, entity: Entity) -> Vec<&'static str> {
        self.stores.values().filter(|s| s.has(entity)).map(|s| s.name()).collect()
    }

    pub fn insert_resource<R: 'static>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    pub fn resource<R: 'static>(&self) -> Option<&R> {
        self.resources.get(&TypeId::of::<R>())?.downcast_ref::<R>()
    }

    pub fn resource_mut<R: 'static>(&mut self) -> Option<&mut R> {
        self.resources.get_mut(&TypeId::of::<R>())?.downcast_mut::<R>()
    }

    fn typed<T: 'static>(&self) -> Option<&Store<T>> {
        self.stores.get(&TypeId::of::<T>())?.as_any().downcast_ref::<Store<T>>()
    }

    fn typed_mut<T: 'static>(&mut self) -> Option<&mut Store<T>> {
        self.stores.get_mut(&TypeId::of::<T>())?.as_any_mut().downcast_mut::<Store<T>>()
    }
}

/// A system: a function over the world. Systems run in the order they were added.
pub type System = fn(&mut World);

/// An ordered list of systems.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(&'static str, System)>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &'static str, system: System) {
        self.systems.push((name, system));
    }

    /// The system names in run order.
    pub fn names(&self) -> Vec<&'static str> {
        self.systems.iter().map(|(name, _)| *name).collect()
    }

    pub fn run(&self, world: &mut World) {
        for (_, system) in &self.systems {
            system(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Position(f32);
    #[derive(Debug, PartialEq)]
    struct Speed(f32);

    #[test]
    fn components_are_per_entity() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.insert(a, Position(1.0)).unwrap();
        world.insert(b, Position(2.0)).unwrap();
        world.insert(b, Speed(5.0)).unwrap();
        assert_eq!(world.get::<Position>(a), Some(&Position(1.0)));
        assert_eq!(world.get::<Speed>(a), None);
        assert_eq!(world.query::<Position>().len(), 2);
        assert_eq!(world.component_names(b).len(), 2);
    }

    #[test]
    fn despawn_invalidates_the_handle_and_reuse_gets_a_new_generation() {
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Position(1.0)).unwrap();
        assert!(world.despawn(a));
        assert!(!world.is_alive(a));
        assert!(world.get::<Position>(a).is_none());
        assert!(!world.despawn(a), "a stale handle cannot despawn twice");
        let b = world.spawn();
        assert_eq!(b.index(), a.index());
        assert_ne!(b, a, "the reused slot has a new generation");
        assert!(world.get::<Position>(b).is_none(), "components are not inherited by the new entity");
    }

    #[test]
    fn stale_entities_cannot_be_given_components() {
        let mut world = World::new();
        let a = world.spawn();
        world.despawn(a);
        assert!(world.insert(a, Position(0.0)).is_err());
    }

    #[test]
    fn ids_round_trip_through_text() {
        let e = Entity { index: 7, generation: 2 };
        assert_eq!(e.to_string(), "7.2");
        assert_eq!("7.2".parse::<Entity>().unwrap(), e);
        assert!("seven".parse::<Entity>().is_err());
    }

    #[test]
    fn systems_run_in_order_and_see_each_others_writes() {
        fn integrate(world: &mut World) {
            let dt = world.resource::<f32>().copied().unwrap_or(0.0);
            for e in world.with::<Speed>() {
                let speed = world.get::<Speed>(e).unwrap().0;
                world.get_mut::<Position>(e).unwrap().0 += speed * dt;
            }
        }
        fn double(world: &mut World) {
            for e in world.with::<Position>() {
                world.get_mut::<Position>(e).unwrap().0 *= 2.0;
            }
        }
        let mut world = World::new();
        world.insert_resource(0.5f32);
        let e = world.spawn();
        world.insert(e, Position(1.0)).unwrap();
        world.insert(e, Speed(2.0)).unwrap();
        let mut schedule = Schedule::new();
        schedule.add("integrate", integrate);
        schedule.add("double", double);
        assert_eq!(schedule.names(), ["integrate", "double"]);
        schedule.run(&mut world);
        // (1 + 2 × 0.5) × 2 = 4
        assert_eq!(world.get::<Position>(e), Some(&Position(4.0)));
    }

    #[test]
    fn despawn_removes_every_component_store_entry() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position(1.0)).unwrap();
        world.insert(e, Speed(1.0)).unwrap();
        world.despawn(e);
        assert!(world.query::<Position>().is_empty());
        assert!(world.query::<Speed>().is_empty());
        assert!(world.entities().is_empty());
    }
}
