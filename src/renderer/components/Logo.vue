<template>
  <div id="header" :style="positionStyle">
    <div class="logo">
      <div class="inner"></div>
    </div>
    <div class="title"><span>HLSR</span><span class="header__version">BETA</span></div>
  </div>
</template>

<script>
export default {
  name: "NavBarHeader",
  props: ["absolute"],
  computed: {
    positionStyle() {
      return "position: " + (this.absolute ? "absolute" : "inherit");
    },
  },
};
</script>

<style scoped>
#header {
  padding: 32px;
  display: flex;
  align-items: center;
}
.logo {
  width: 48px;
  height: 48px;
  background: transparent;
  background-size: cover;
  border-radius: 50%;
  border: 4px solid;
  color: #fff;
  position: relative;
  transition: 0.5s color;
}
.logo .inner {
  width: 20px;
  height: 20px;
  background: white;
  border-radius: 50%;
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  transition: 0.2s width, 0.2s height, 0.5s background;
}
.logo:hover .inner {
  width: 26px;
  height: 26px;
}
/* Название и «BETA» — как в логотипе на сайте (Brand.tsx, site.css):
   те же кегли и разрядка, «BETA» у верха названия, через 0.38 em после «R». */
.title {
  display: flex;
  align-items: center;
  font-size: 22px;
  line-height: 28px;
  color: white;
  margin-left: 16px;
  font-weight: 700;
  letter-spacing: 6px;
}
.header__version {
  align-self: flex-start;
  margin: 2px 0 0 2px;
  color: rgba(255, 255, 255, 0.5);
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
  letter-spacing: 0.08em;
}
</style>
