// The Vue 3 acceptance page /vue of tools/w24-site.py (W59): counter, v-model, v-for, v-if, a check
// box, a select and a form with @submit.prevent. Every change prints the state on the console.
(function () {
  var app = Vue.createApp({
    data: function () {
      return { title: 'Vue ' + Vue.version, count: 0, name: '', fruits: ['banana', 'caju'], fruit: '', show: false,
        color: 'vermelho', msg: '', sent: '' };
    },
    computed: {
      hex: function () { return { vermelho: '#c0392b', verde: '#27ae60', azul: '#2980b9' }[this.color]; },
    },
    methods: {
      addFruit: function () {
        if (this.fruit) { this.fruits.push(this.fruit); this.fruit = ''; }
      },
      send: function () { this.sent = this.msg; this.msg = ''; },
      report: function (why) {
        var self = this;
        this.$nextTick(function () {
          var lis = document.querySelectorAll('.fruits li');
          console.log('vue: ' + why + ' count=' + self.count + ' name=' + JSON.stringify(self.name) + ' fruits=' +
            self.fruits.join('|') + ' dom=' + lis.length + ' show=' + self.show + ' extra=' + !!document.querySelector('.extra') +
            ' color=' + self.color + ' sent=' + JSON.stringify(self.sent) + ' hello=' + JSON.stringify(document.getElementById('hello').textContent) +
            ' countdom=' + document.getElementById('count').textContent);
        });
      },
    },
    watch: {
      count: function () { this.report('count'); document.title = 'Vue: ' + this.count; },
      name: function () { this.report('name'); },
      fruits: { deep: true, handler: function () { this.report('fruits'); } },
      show: function () { this.report('show'); },
      color: function () { this.report('color'); },
      sent: function () { this.report('sent'); },
    },
    mounted: function () {
      this.report('mounted');
      // Where the controls are on screen, whenever the app changes size (after the browser laid the
      // page out): the QEMU scenario clicks there.
      function where(sel) {
        var el = document.querySelector(sel);
        if (!el) return '-';
        var r = el.getBoundingClientRect();
        return Math.round(r.left + Math.min(r.width / 2, 12)) + ',' + Math.round(r.top + r.height / 2);
      }
      new ResizeObserver(function () {
        console.log('vue: at inc=' + where('#inc') + ' name=' + where('#name') + ' fruit=' + where('#fruit') +
          ' show=' + where('#show') + ' select=' + where('select') + ' msg=' + where('form input') +
          ' h=' + Math.round(document.getElementById('app').getBoundingClientRect().height));
      }).observe(document.getElementById('app'));
    },
  });
  app.config.errorHandler = function (e) { console.log('vue: error ' + e + (e && e.stack ? ' ' + e.stack : '')); };
  app.config.warnHandler = function (m) { console.log('vue: warn ' + m); };
  app.mount('#app');
})();
