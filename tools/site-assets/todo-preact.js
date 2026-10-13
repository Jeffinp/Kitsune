// TodoMVC with Preact + htm (the acceptance page /preact of tools/w24-site.py; W59). Every render
// prints its state on the console (the serial log of the QEMU run) so the scenario can check it.
(function () {
  var h = preact.h, render = preact.render;
  var useState = preactHooks.useState, useEffect = preactHooks.useEffect, useRef = preactHooks.useRef;
  var html = htm.bind(h);
  var KEY = 'todos-preact';

  function load() {
    try { return JSON.parse(localStorage.getItem(KEY) || '[]'); } catch (e) { return []; }
  }
  function route() { return (location.hash.replace(/^#\/?/, '') || 'all'); }

  function Item(props) {
    var t = props.todo;
    return html`<li class=${t.done ? 'completed' : ''}>
      <input class="toggle" type="checkbox" checked=${t.done} onChange=${function () { props.onToggle(t.id); }} />
      <label>${t.title}</label>
      <button class="destroy" onClick=${function () { props.onRemove(t.id); }}>remover</button>
    </li>`;
  }

  function App() {
    var s1 = useState(load), todos = s1[0], setTodos = s1[1];
    var s2 = useState(''), text = s2[0], setText = s2[1];
    var s3 = useState(route()), filter = s3[0], setFilter = s3[1];
    var s4 = useState(''), focus = s4[0], setFocus = s4[1];
    var input = useRef(null);
    var nextId = useRef(todos.reduce(function (m, t) { return Math.max(m, t.id); }, 0) + 1);

    useEffect(function () {
      function onHash() { setFilter(route()); }
      addEventListener('hashchange', onHash);
      if (input.current) input.current.focus();
      // Where the controls are on screen, whenever the list changes size (after the browser laid the
      // page out): the QEMU scenario clicks there.
      function where(sel, i) {
        var el = document.querySelectorAll(sel)[i || 0];
        if (!el) return '-';
        var r = el.getBoundingClientRect();
        return Math.round(r.left + Math.min(r.width / 2, 12)) + ',' + Math.round(r.top + r.height / 2);
      }
      var ro = new ResizeObserver(function () {
        console.log('todo: at input=' + where('.new-todo') + ' toggle2=' + where('.toggle', 1) + ' destroy1=' +
          where('.destroy', 0) + ' all=' + where('.filters a', 0) + ' active=' + where('.filters a', 1) +
          ' items=' + document.querySelectorAll('.todo-list li').length);
      });
      ro.observe(document.querySelector('.todoapp'));
      return function () { removeEventListener('hashchange', onHash); ro.disconnect(); };
    }, []);

    var left = todos.filter(function (t) { return !t.done; }).length;
    var shown = todos.filter(function (t) { return filter === 'active' ? !t.done : filter === 'completed' ? t.done : true; });

    useEffect(function () {
      localStorage.setItem(KEY, JSON.stringify(todos));
      document.title = 'Preact: ' + left + ' restantes';
      var lis = document.querySelectorAll('.todo-list li');
      console.log('todo: n=' + todos.length + ' left=' + left + ' filter=' + filter + ' shown=' +
        shown.map(function (t) { return t.title + (t.done ? '+' : ''); }).join('|') + ' dom=' + lis.length +
        ' input=' + JSON.stringify(text) + ' focus=' + focus);
    });

    function onKeyDown(e) {
      if (e.key === 'Enter') {
        var v = e.target.value.trim();
        console.log('todo: enter value=' + JSON.stringify(v));
        if (v) {
          setTodos(todos.concat([{ id: nextId.current++, title: v, done: false }]));
          setText('');
        }
      }
    }
    function toggle(id) {
      setTodos(todos.map(function (t) { return t.id === id ? { id: t.id, title: t.title, done: !t.done } : t; }));
    }
    function remove(id) { setTodos(todos.filter(function (t) { return t.id !== id; })); }
    function clearDone() { setTodos(todos.filter(function (t) { return !t.done; })); }

    return html`<section class="todoapp">
      <header>
        <h1>todos</h1>
        <input ref=${input} class="new-todo" size="40" placeholder="O que precisa ser feito?" value=${text}
          onInput=${function (e) { setText(e.target.value); }} onKeyDown=${onKeyDown}
          onFocus=${function () { setFocus('in'); }} onBlur=${function () { setFocus('out'); }} />
      </header>
      <ul class="todo-list">${shown.map(function (t) { return html`<${Item} key=${t.id} todo=${t} onToggle=${toggle} onRemove=${remove} />`; })}</ul>
      <footer class="footer">
        <span class="todo-count"><strong>${left}</strong> ${left === 1 ? 'item restante' : 'itens restantes'}</span>
        <ul class="filters">
          <li><a href="#/" class=${filter === 'all' ? 'selected' : ''}>Todos</a></li>
          <li><a href="#/active" class=${filter === 'active' ? 'selected' : ''}>Ativos</a></li>
          <li><a href="#/completed" class=${filter === 'completed' ? 'selected' : ''}>Concluidos</a></li>
        </ul>
        <button class="clear-completed" onClick=${clearDone}>Limpar concluidos</button>
      </footer>
      <p class="info">Campo: ${focus === 'in' ? 'com foco' : 'sem foco'}; digitado: ${text.length} letras.</p>
    </section>`;
  }

  console.log('todo: preact ' + (typeof preact.render) + ' hooks ' + (typeof preactHooks.useState) + ' htm ' + (typeof htm));
  render(html`<${App} />`, document.getElementById('app'));
})();
